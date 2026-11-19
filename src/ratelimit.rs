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

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
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

pub struct RateLimiter {
    path: Option<PathBuf>,
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

impl RateLimiter {
    /// Kept in `path` (none: in memory only), waiting at most `max_wait`.
    pub fn new(path: Option<PathBuf>, max_wait: Duration, note: Note) -> Self {
        let window = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        RateLimiter {
            path,
            window: Mutex::new(window),
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

    pub fn window(&self) -> Window {
        self.window.lock().map(|w| w.clone()).unwrap_or_default()
    }

    /// Wait, if the window has nothing left, or refuse, after a 429.
    pub fn before(&self) -> Result<(), CliError> {
        let now = (self.clock)();
        let wait = {
            let w = self.window.lock().map_err(poisoned)?;
            if let Some(until) = w.blocked_until.filter(|u| *u > now) {
                return Err(CliError::Upstream(format!(
                    "ICANotes refuses requests for fifteen minutes after a 429 (too many requests); {} left",
                    span(until - now)
                ))
                .because(RATE_LIMITED)
                .with_remedy(format!("try again in {}", span(until - now + 1))));
            }
            match (w.remaining, w.reset) {
                (Some(0), Some(reset)) if reset >= now => Some(reset - now + 1),
                _ => None,
            }
        };
        if let Some(secs) = wait {
            let d = Duration::from_secs(secs as u64);
            if d > self.max_wait {
                return Err(CliError::Upstream(format!(
                    "ICANotes' request window is used up for {}, longer than the {} this run may wait",
                    span(secs),
                    span(self.max_wait.as_secs() as i64)
                ))
                .because(RATE_LIMITED)
                .with_remedy(format!("try again in {}, or pass a longer --max-wait", span(secs))));
            }
            let limit = self.window().limit.unwrap_or(10);
            (self.note)(&format!(
                "ICANotes allows {limit} requests per window; waiting {} for the next one",
                span(secs)
            ));
            (self.sleep)(d);
            if let Ok(mut w) = self.window.lock() {
                w.remaining = None;
                w.reset = None;
            }
        }
        Ok(())
    }

    /// Read the window from an answer's headers (names lower case).
    pub fn after(&self, status: u16, headers: &[(String, String)]) {
        let now = (self.clock)();
        let header = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.trim().to_string())
        };
        let num = |name: &str| header(name).and_then(|v| v.parse::<i64>().ok());
        if let Ok(mut w) = self.window.lock() {
            let remaining = num("x-ratelimit-remaining");
            if let Some(l) = num("x-ratelimit-limit") {
                w.limit = u32::try_from(l).ok();
            }
            if let Some(r) = num("x-ratelimit-reset") {
                // Unix seconds as ICANotes sends it; a small number is
                // seconds from now, as some servers send it.
                w.reset = Some(if r > 1_000_000_000 { r } else { now + r });
            }
            match remaining {
                Some(r) => w.remaining = u32::try_from(r.max(0)).ok(),
                None => {
                    if w.reset.is_some_and(|reset| reset >= now) {
                        w.remaining = w.remaining.map(|r| r.saturating_sub(1));
                    }
                }
            }
            if status == 429 {
                let asked = header("retry-after")
                    .and_then(|v| v.parse::<i64>().ok())
                    .map(|s| now + s);
                let until = [Some(now + PENALTY_SECS), asked]
                    .into_iter()
                    .flatten()
                    .max()
                    .unwrap_or(now + PENALTY_SECS);
                w.blocked_until = Some(until);
                w.remaining = Some(0);
            } else if w.blocked_until.is_some_and(|u| u <= now) {
                w.blocked_until = None;
            }
            self.save(&w);
        }
    }

    fn save(&self, w: &Window) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = pk_cli_core::private_file::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec(w) {
            let _ = pk_cli_core::private_file::write(
                path,
                &bytes,
                pk_cli_core::private_file::Replace::Always,
            );
        }
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
    fn without_headers_a_known_window_counts_down() {
        let now = Arc::new(AtomicI64::new(T0));
        let slept = Arc::new(AtomicI64::new(0));
        let r = limiter(now, slept, None);
        r.after(200, &headers(2, T0 + 60));
        r.after(200, &[]);
        assert_eq!(r.window().remaining, Some(1));
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
        let next = limiter(now, slept.clone(), Some(path));
        next.before().unwrap();
        assert_eq!(slept.load(Ordering::SeqCst), 31);
    }

    #[test]
    fn spans_read_as_minutes_and_seconds() {
        assert_eq!(span(45), "45s");
        assert_eq!(span(600), "10m");
        assert_eq!(span(581), "9m 41s");
        assert_eq!(span(-3), "0s");
    }
}
