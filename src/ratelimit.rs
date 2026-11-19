//! ICANotes' request window. The API answers `X-RateLimit-Limit` (10),
//! `X-RateLimit-Remaining` and `X-RateLimit-Reset` (Unix seconds) on every
//! request, and an "abuse" answer is a 429 and fifteen minutes refused. One
//! export makes 13 requests, so it must wait out a window rather than run
//! into a 429.
//!
//! The window is kept in `rate.json` beside the config, so one run's
//! requests count against the next: `auth set-credential` then `records
//! export` share a window. Before each request, a window with nothing left
//! is waited out (announced on stderr); after a 429, nothing is sent until
//! ICANotes' fifteen minutes are over.

use std::fs::File;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use pk_cli_core::CliError;
use serde::{Deserialize, Serialize};

/// The cause of every refusal over ICANotes' request window: a 429, the
/// fifteen minutes after one, a wait longer than the run allows. No later
/// request in the same run can succeed, so an export ends on it.
pub const RATE_LIMITED: pk_cli_core::Reason = pk_cli_core::Reason::Provider("rate-limited");

/// What ICANotes documents a 429 costs.
pub const PENALTY_SECS: i64 = 15 * 60;
/// The longest a request waits for a window by default: a little over the
/// window seen in practice.
pub const DEFAULT_MAX_WAIT: Duration = Duration::from_secs(16 * 60);
/// How long a window is taken to last when an answer says it is used up
/// but not when it resets: a little over the ten-odd minutes seen.
pub const ASSUMED_WINDOW_SECS: i64 = 11 * 60;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Requests left in the window: ICANotes' last count, less the requests
    /// sent since, by this run or another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<u32>,
    /// When the window resets, Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<i64>,
    /// After a 429: nothing is sent before this, Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_until: Option<i64>,
}

pub type Clock = Box<dyn Fn() -> i64 + Send + Sync>;
pub type Sleep = Box<dyn Fn(Duration) + Send + Sync>;
pub type Note = Box<dyn Fn(&str) + Send + Sync>;

/// The request window, shared by every icanotes run through `rate.json`.
///
/// Each check and each update re-reads the file under a lock on
/// `rate.lock` beside it, so two runs at once (two profiles' exports, an
/// export and an `auth status --check`) count against one window, and a
/// 429 one of them met holds the other back. A request takes its slot
/// before it is sent.
pub struct RateLimiter {
    path: Option<PathBuf>,
    /// The window when there is no file (tests), and the last one read.
    window: Mutex<Window>,
    max_wait: Duration,
    clock: Clock,
    sleep: Sleep,
    note: Note,
}

impl std::fmt::Debug for RateLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimiter")
            .field("path", &self.path)
            .field("window", &self.window)
            .finish_non_exhaustive()
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What [`RateLimiter::before`] decided under the lock.
enum Turn {
    Go,
    Wait(i64),
}

impl RateLimiter {
    /// Kept in `path` (none: in memory only), waiting at most `max_wait`.
    pub fn new(path: Option<PathBuf>, max_wait: Duration, note: Note) -> Self {
        RateLimiter {
            path,
            window: Mutex::new(Window::default()),
            max_wait,
            clock: Box::new(now),
            sleep: Box::new(std::thread::sleep),
            note,
        }
    }

    /// A clock and sleep of the caller's own (tests).
    pub fn with_time(mut self, clock: Clock, sleep: Sleep) -> Self {
        self.clock = clock;
        self.sleep = sleep;
        self
    }

    /// The window as last read or written.
    pub fn window(&self) -> Window {
        self.window.lock().map(|w| w.clone()).unwrap_or_default()
    }

    /// Run `f` on the current window under the lock, and keep what it
    /// leaves. Without a file, the lock is this process's.
    fn update<T>(&self, f: impl FnOnce(&mut Window) -> T) -> Result<T, CliError> {
        let mut cached = self.window.lock().map_err(poisoned)?;
        let Some(path) = &self.path else {
            return Ok(f(&mut cached));
        };
        if let Some(dir) = path.parent() {
            pk_cli_core::private_file::create_dir_all(dir)
                .map_err(|e| CliError::Other(format!("creating {}: {e}", dir.display())))?;
        }
        let lock_path = path.with_extension("lock");
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|e| CliError::Other(format!("opening {}: {e}", lock_path.display())))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600));
        }
        lock.lock()
            .map_err(|e| CliError::Other(format!("locking {}: {e}", lock_path.display())))?;
        let mut w: Window = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let before = w.clone();
        let out = f(&mut w);
        if w != before {
            if let Ok(bytes) = serde_json::to_vec(&w) {
                let _ = pk_cli_core::private_file::write(
                    path,
                    &bytes,
                    pk_cli_core::private_file::Replace::Always,
                );
            }
        }
        *cached = w;
        drop(lock);
        Ok(out)
    }

    /// Take a slot in the window for one request: wait while it is used
    /// up, and refuse during the fifteen minutes after a 429 or when the
    /// wait would be longer than this run may wait.
    pub fn before(&self) -> Result<(), CliError> {
        let mut announced = false;
        loop {
            let now = (self.clock)();
            let turn = self.update(|w| -> Result<Turn, CliError> {
                if let Some(until) = w.blocked_until.filter(|u| *u > now) {
                    return Err(CliError::Upstream(format!(
                        "ICANotes refuses requests for fifteen minutes after a 429 (too many requests); {} left",
                        span(until - now)
                    ))
                    .because(RATE_LIMITED)
                    .with_remedy(format!("try again in {}", span(until - now + 1))));
                }
                if w.reset.is_some_and(|r| r < now) {
                    // That window is over; the next answer gives the new one.
                    w.remaining = None;
                    w.reset = None;
                }
                match (w.remaining, w.reset) {
                    (Some(0), Some(reset)) => Ok(Turn::Wait(reset - now + 1)),
                    (Some(0), None) => {
                        // Used up with no reset known: count the window
                        // from now.
                        w.reset = Some(now + ASSUMED_WINDOW_SECS);
                        Ok(Turn::Wait(ASSUMED_WINDOW_SECS + 1))
                    }
                    (Some(left), _) => {
                        w.remaining = Some(left - 1);
                        Ok(Turn::Go)
                    }
                    (None, _) => Ok(Turn::Go),
                }
            })??;
            let secs = match turn {
                Turn::Go => return Ok(()),
                Turn::Wait(secs) => secs.max(1),
            };
            let d = Duration::from_secs(secs as u64);
            if d > self.max_wait {
                return Err(CliError::Upstream(format!(
                    "ICANotes' request window is used up for {}, longer than the {} this run may wait",
                    span(secs),
                    span(self.max_wait.as_secs() as i64)
                ))
                .because(RATE_LIMITED)
                .with_remedy(format!(
                    "try again in {}, or pass a longer --max-wait",
                    span(secs)
                )));
            }
            if !announced {
                let limit = self.window().limit.unwrap_or(10);
                (self.note)(&format!(
                    "ICANotes allows {limit} requests per window; waiting {} for the next one",
                    span(secs)
                ));
                announced = true;
            }
            (self.sleep)(d);
        }
    }

    /// Read the window from an answer's headers (names in any case). The
    /// request's slot was taken in [`before`](Self::before), so an answer
    /// with no count leaves the count as it is.
    pub fn after(&self, status: u16, headers: &[(String, String)]) {
        let now = (self.clock)();
        let header = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.trim().to_string())
        };
        let num = |name: &str| header(name).and_then(|v| v.parse::<i64>().ok());
        let _ = self.update(|w| {
            if let Some(l) = num("x-ratelimit-limit") {
                w.limit = u32::try_from(l).ok();
            }
            if let Some(r) = num("x-ratelimit-reset") {
                // Unix seconds as ICANotes sends it; a small number is
                // seconds from now, as some servers send it.
                w.reset = Some(if r > 1_000_000_000 { r } else { now + r });
            }
            if let Some(r) = num("x-ratelimit-remaining") {
                w.remaining = u32::try_from(r.max(0)).ok();
            }
            if w.remaining == Some(0) && !w.reset.is_some_and(|r| r >= now) {
                w.reset = Some(now + ASSUMED_WINDOW_SECS);
            }
            if status == 429 {
                let asked = header("retry-after")
                    .and_then(|v| v.parse::<i64>().ok())
                    .map(|s| now + s);
                let until = [Some(now + PENALTY_SECS), asked, w.blocked_until]
                    .into_iter()
                    .flatten()
                    .max()
                    .unwrap_or(now + PENALTY_SECS);
                w.blocked_until = Some(until);
                w.remaining = Some(0);
            } else if w.blocked_until.is_some_and(|u| u <= now) {
                w.blocked_until = None;
            }
        });
    }
}

fn poisoned<T>(_: T) -> CliError {
    CliError::Other("the request window's lock was poisoned".into())
}

/// `9m 40s`, `45s`.
pub fn span(secs: i64) -> String {
    let secs = secs.max(0);
    match (secs / 60, secs % 60) {
        (0, s) => format!("{s}s"),
        (m, 0) => format!("{m}m"),
        (m, s) => format!("{m}m {s}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;

    fn limiter(now: Arc<AtomicI64>, slept: Arc<AtomicI64>, path: Option<PathBuf>) -> RateLimiter {
        let n = now.clone();
        RateLimiter::new(path, DEFAULT_MAX_WAIT, Box::new(|_| {})).with_time(
            Box::new(move || n.load(Ordering::SeqCst)),
            Box::new(move |d| {
                slept.fetch_add(d.as_secs() as i64, Ordering::SeqCst);
                now.fetch_add(d.as_secs() as i64, Ordering::SeqCst);
            }),
        )
    }

    fn headers(remaining: i64, reset: i64) -> Vec<(String, String)> {
        vec![
            ("x-ratelimit-limit".into(), "10".into()),
            ("x-ratelimit-remaining".into(), remaining.to_string()),
            ("x-ratelimit-reset".into(), reset.to_string()),
        ]
    }

    const T0: i64 = 1_800_000_000;

    #[test]
    fn a_used_up_window_is_waited_out() {
        let now = Arc::new(AtomicI64::new(T0));
        let slept = Arc::new(AtomicI64::new(0));
        let r = limiter(now.clone(), slept.clone(), None);
        r.before().unwrap();
        r.after(200, &headers(1, T0 + 600));
        r.before().unwrap();
        assert_eq!(slept.load(Ordering::SeqCst), 0);
        r.after(200, &headers(0, T0 + 600));
        r.before().unwrap();
        assert_eq!(slept.load(Ordering::SeqCst), 601);
        // A fresh window: nothing to wait for.
        r.before().unwrap();
        assert_eq!(slept.load(Ordering::SeqCst), 601);
    }

    #[test]
    fn a_wait_longer_than_allowed_is_an_upstream_error() {
        let now = Arc::new(AtomicI64::new(T0));
        let slept = Arc::new(AtomicI64::new(0));
        let r = limiter(now, slept.clone(), None);
        r.after(200, &headers(0, T0 + 3600));
        let e = r.before().unwrap_err();
        assert_eq!(e.exit_code(), 5);
        assert_eq!(slept.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_429_blocks_for_fifteen_minutes() {
        let now = Arc::new(AtomicI64::new(T0));
        let slept = Arc::new(AtomicI64::new(0));
        let r = limiter(now.clone(), slept, None);
        r.after(429, &[]);
        let e = r.before().unwrap_err();
        assert_eq!(e.exit_code(), 5);
        assert!(e.to_string().contains("15m"), "{e}");
        now.store(T0 + PENALTY_SECS + 1, Ordering::SeqCst);
        r.before().unwrap();
    }

    #[test]
    fn each_request_takes_its_slot_before_it_is_sent() {
        let now = Arc::new(AtomicI64::new(T0));
        let slept = Arc::new(AtomicI64::new(0));
        let r = limiter(now, slept.clone(), None);
        r.after(200, &headers(2, T0 + 60));
        r.before().unwrap();
        // An answer with no count leaves the slot taken.
        r.after(200, &[]);
        assert_eq!(r.window().remaining, Some(1));
        r.before().unwrap();
        assert_eq!(r.window().remaining, Some(0));
        r.before().unwrap();
        assert_eq!(slept.load(Ordering::SeqCst), 61);
    }

    #[test]
    fn a_used_up_window_with_no_reset_is_waited_out_too() {
        let now = Arc::new(AtomicI64::new(T0));
        let slept = Arc::new(AtomicI64::new(0));
        let r = limiter(now, slept.clone(), None);
        r.after(
            200,
            &[
                ("X-RateLimit-Limit".into(), "10".into()),
                ("X-RateLimit-Remaining".into(), "0".into()),
            ],
        );
        r.before().unwrap();
        assert_eq!(slept.load(Ordering::SeqCst), ASSUMED_WINDOW_SECS + 1);
    }

    #[test]
    fn two_runs_share_one_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rate.json");
        let now = Arc::new(AtomicI64::new(T0));
        let slept = Arc::new(AtomicI64::new(0));
        let a = limiter(now.clone(), slept.clone(), Some(path.clone()));
        let b = limiter(now.clone(), slept.clone(), Some(path.clone()));
        a.after(200, &headers(1, T0 + 100));
        // The last slot goes to whichever run asks first.
        a.before().unwrap();
        assert_eq!(slept.load(Ordering::SeqCst), 0);
        b.before().unwrap();
        assert_eq!(slept.load(Ordering::SeqCst), 101);
    }

    #[test]
    fn a_429_one_run_met_holds_the_other_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rate.json");
        let now = Arc::new(AtomicI64::new(T0));
        let slept = Arc::new(AtomicI64::new(0));
        let a = limiter(now.clone(), slept.clone(), Some(path.clone()));
        let b = limiter(now.clone(), slept.clone(), Some(path.clone()));
        a.after(429, &[]);
        // b's answer to a request sent before the 429 does not clear it.
        b.after(200, &headers(5, T0 + 100));
        assert_eq!(b.before().unwrap_err().exit_code(), 5);
        assert_eq!(a.before().unwrap_err().exit_code(), 5);
    }

    #[test]
    fn a_relative_reset_is_seconds_from_now() {
        let now = Arc::new(AtomicI64::new(T0));
        let slept = Arc::new(AtomicI64::new(0));
        let r = limiter(now, slept, None);
        r.after(200, &headers(3, 120));
        assert_eq!(r.window().reset, Some(T0 + 120));
    }

    #[test]
    fn the_window_carries_over_to_the_next_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rate.json");
        let now = Arc::new(AtomicI64::new(T0));
        let slept = Arc::new(AtomicI64::new(0));
        limiter(now.clone(), slept.clone(), Some(path.clone())).after(200, &headers(0, T0 + 30));
        let next = limiter(now, slept.clone(), Some(path.clone()));
        next.before().unwrap();
        assert_eq!(slept.load(Ordering::SeqCst), 31);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for f in [path.clone(), path.with_extension("lock")] {
                let mode = std::fs::metadata(&f).unwrap().permissions().mode();
                assert_eq!(mode & 0o077, 0, "{}", f.display());
            }
        }
    }

    #[test]
    fn spans_read_as_minutes_and_seconds() {
        assert_eq!(span(45), "45s");
        assert_eq!(span(600), "10m");
        assert_eq!(span(581), "9m 41s");
        assert_eq!(span(-3), "0s");
    }
}
