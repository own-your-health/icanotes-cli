//! `auth login --browser`: the patient portal in Chrome, in the profile's own
//! kept browser profile (cli-common `pk-cli-browser`). The patient (or a
//! session filling the sign-in for them) signs in and generates a key on My
//! History, FHIR API Access; icanotes reads the key and secret off that page,
//! checks them with one `PatientID` request before the tab closes, and
//! stores them. Neither is printed.
//!
//! The crate never drives the page and never touches the sign-in form: it
//! lists the window's pages and evaluates [`KEY_EXPRESSION`] in those on the
//! portal's host until it answers.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use pk_cli_browser::{Capture, Captured, Confirm, HostRule, Launch};
use pk_cli_config::profile::ProfileName;
use pk_cli_core::CliError;
use serde::Deserialize;

use crate::credential::KeyPair;

/// Reads the key and secret the FHIR API Access page shows after "Generate
/// New Credentials", as a JSON string `{"key": …, "secret": …}`, or `null`
/// while the page shows none. It looks for the values the page labels as
/// the user key and the user secret (an input's value or the text beside
/// its label), and answers only when both are there.
///
/// A value is at least 8 printable characters with no space. Buttons,
/// links and labels are never read as values, and neither is a value made
/// only of the page's own words ("UserSecret", "Generate", "Copy key"), so
/// a label the page names `lblUserSecret` is passed over for the value
/// beside it.
///
/// When the practice has turned the portal off for the login, the sign-in
/// answers "Use of the Patient Portal has been disabled" instead of
/// signing in. The expression then answers `{"stopped": "portal-disabled"}`,
/// so the wait ends at once rather than at its timeout.
pub const KEY_EXPRESSION: &str = r#"(() => {
  const norm = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const shown = norm(document.body ? document.body.innerText : '');
  if (/use of the patient portal has been disabled/i.test(shown)) {
    return JSON.stringify({ stopped: 'portal-disabled' });
  }
  const UI = /^(?:user|api|client|patient|portal|fhir|access|new|your|generate|generated|regenerate|copy|copied|show|hide|reveal|delete|remove|credentials?|keys?|secrets?|tokens?|value|here|[\s:_\-.])+$/i;
  const NOT_VALUES = 'button, a, option, select, label, th, dt, [role=button], [role=link]';
  const valueOf = (el) => {
    if (!el || (el.matches && el.matches(NOT_VALUES))) return '';
    if ('value' in el && typeof el.value === 'string' && el.value.trim()) return el.value.trim();
    return norm(el.textContent);
  };
  const looksLike = (v) => /^[\x21-\x7e]{8,1000}$/.test(v) && !UI.test(v);
  const find = (re) => {
    // An input or element whose id, name or label names it.
    for (const el of document.querySelectorAll('input, textarea, [id], [name]')) {
      const tag = [el.id, el.getAttribute('name'), el.getAttribute('aria-label')].join(' ');
      if (re.test(tag)) {
        const v = valueOf(el);
        if (looksLike(v)) return v;
      }
    }
    for (const label of document.querySelectorAll('label, th, td, dt, span, strong, b, p, div')) {
      if (label.children.length > 2) continue;
      const t = norm(label.textContent);
      if (!re.test(t) || t.length > 40) continue;
      const forId = label.getAttribute && label.getAttribute('for');
      const target = (forId && document.getElementById(forId))
        || label.nextElementSibling
        || (label.parentElement && label.parentElement.nextElementSibling);
      const v = valueOf(target);
      if (looksLike(v)) return v;
      const after = t.replace(re, '').replace(/^[\s:]+/, '');
      if (looksLike(after)) return after;
    }
    return '';
  };
  const key = find(/user\s*key|api\s*key|client\s*key|^\s*key\s*:?\s*$/i);
  const secret = find(/user\s*secret|api\s*secret|client\s*secret|^\s*secret\s*:?\s*$/i);
  if (!key || !secret || key === secret) return null;
  return JSON.stringify({ key, secret });
})()"#;

/// What the window is asked to do.
pub struct WindowRequest<'a> {
    pub profile: &'a ProfileName,
    /// Where the window opens.
    pub url: &'a str,
    /// The host the key is read from.
    pub host: &'a str,
    pub timeout: Duration,
    /// Decides whether a candidate key works (one `PatientID` request).
    pub confirm: Arc<dyn Fn(&KeyPair) -> Confirm + Send + Sync>,
}

/// Opens the portal and returns the key once `confirm` accepts it.
pub type Window = Box<dyn Fn(&WindowRequest) -> Result<KeyPair, CliError>>;

#[derive(Deserialize)]
#[serde(untagged)]
enum Shown {
    Pair { key: String, secret: String },
    Stopped { stopped: String },
}

/// What the page answered.
#[derive(Debug)]
pub enum Answer {
    /// The key and secret the FHIR API Access page shows.
    Pair(KeyPair),
    /// The sign-in said the practice has turned the portal off.
    PortalDisabled,
}

/// The page's answer: a key and secret, or a sign-in no later poll changes.
pub fn read_answer(text: &str) -> Result<Answer, CliError> {
    let unread =
        || CliError::Upstream("the FHIR API Access page's key and secret did not read".into());
    match serde_json::from_str(text).map_err(|_| unread())? {
        Shown::Pair { key, secret } => KeyPair::new(&key, &secret).map(Answer::Pair),
        Shown::Stopped { stopped } if stopped == "portal-disabled" => Ok(Answer::PortalDisabled),
        Shown::Stopped { .. } => Err(unread()),
    }
}

/// The page's answer as a key and secret.
pub fn read_shown(text: &str) -> Result<KeyPair, CliError> {
    match read_answer(text)? {
        Answer::Pair(pair) => Ok(pair),
        Answer::PortalDisabled => Err(portal_disabled("<NAME>")),
    }
}

/// The error for a portal the practice has turned off: signing in again
/// cannot help, so it ends the wait.
pub fn portal_disabled(profile: &str) -> CliError {
    CliError::Auth(
        "the patient portal answered \"Use of the Patient Portal has been disabled\" at sign-in, \
         so no key can be generated"
            .into(),
    )
    .with_remedy(format!(
        "ask the practice to turn the patient portal back on for this patient, then run \
         `{} auth login --browser --profile {profile}` again ({})",
        crate::commands::BIN,
        crate::commands::SETUP_DOC
    ))
}

/// Chrome, through pk-cli-browser.
pub fn chrome_window() -> Window {
    Box::new(|req: &WindowRequest| {
        let confirm = req.confirm.clone();
        let profile = req.profile.as_str().to_string();
        let capture = Capture::new("ICANotes patient portal", HostRule::exact(req.host)?)
            .expression(KEY_EXPRESSION)
            .confirm(
                move |c: &Captured| match c.value().map(|v| read_answer(v.expose())) {
                    Some(Ok(Answer::Pair(pair))) => confirm(&pair),
                    Some(Ok(Answer::PortalDisabled)) => Confirm::Fail(portal_disabled(&profile)),
                    Some(Err(_)) => {
                        Confirm::NotYet("the page's key and secret did not read".into())
                    }
                    None => Confirm::NotYet("no key on the page yet".into()),
                },
            )
            .hint(
                "the portal password, then My History, FHIR API Access, Generate New Credentials",
            );
        let captured = pk_cli_browser::capture(
            Launch::new(crate::commands::BIN)
                .url(req.url)
                .profile_name(req.profile.as_str()),
            &capture,
            req.timeout,
        )?;
        let value = captured
            .into_value()
            .ok_or_else(|| CliError::Other("the browser wait ended without the key".into()))?;
        read_shown(value.expose())
    })
}

/// Remembers each candidate's verdict, so a key the page keeps showing is
/// asked about once, not on every poll: each ask is a request against
/// ICANotes' window of ten.
#[derive(Default)]
pub struct Verdicts {
    seen: Mutex<Vec<(String, Verdict)>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// ICANotes tied it to this patient id.
    Works(String),
    Refused(String),
}

impl Verdicts {
    fn fingerprint(pair: &KeyPair) -> String {
        use sha2::{Digest, Sha256};
        let d = Sha256::digest(format!("{}\n{}", pair.user_key, pair.user_secret).as_bytes());
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn get(&self, pair: &KeyPair) -> Option<Verdict> {
        let f = Self::fingerprint(pair);
        self.seen
            .lock()
            .ok()?
            .iter()
            .find(|(k, _)| *k == f)
            .map(|(_, v)| v.clone())
    }

    pub fn put(&self, pair: &KeyPair, v: Verdict) {
        if let Ok(mut s) = self.seen.lock() {
            s.push((Self::fingerprint(pair), v));
        }
    }

    /// The patient id the accepted key was tied to.
    pub fn accepted(&self, pair: &KeyPair) -> Option<String> {
        match self.get(pair) {
            Some(Verdict::Works(id)) => Some(id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_answer_reads_as_a_pair() {
        let p = read_shown(r#"{"key":"key-one","secret":"secret-one"}"#).unwrap();
        assert_eq!(p.user_key, "key-one");
        assert!(read_shown("null").is_err());
        assert!(matches!(
            read_answer(r#"{"stopped":"portal-disabled"}"#),
            Ok(Answer::PortalDisabled)
        ));
        assert!(read_answer(r#"{"stopped":"something-else"}"#).is_err());
        let e = portal_disabled("adult1");
        assert_eq!(e.exit_code(), 3, "{e}");
        assert!(e.remedy().unwrap().contains("--profile adult1"), "{e}");
        assert!(read_shown(r#"{"key":"has space","secret":"x"}"#).is_err());
    }

    #[test]
    fn verdicts_are_remembered_per_pair() {
        let v = Verdicts::default();
        let a = KeyPair::new("key-one", "secret-one").unwrap();
        let b = KeyPair::new("key-one", "secret-two").unwrap();
        assert!(v.get(&a).is_none());
        v.put(&a, Verdict::Works("1001".into()));
        v.put(&b, Verdict::Refused("403".into()));
        assert_eq!(v.accepted(&a).as_deref(), Some("1001"));
        assert!(v.accepted(&b).is_none());
    }
}
