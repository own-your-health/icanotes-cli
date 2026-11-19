//! A patient's API key and secret, as the portal's "FHIR API Access" page
//! generates them: one vault entry per (profile, patient),
//! `<profile>/icanotes/<patient id>`, in icanotes' one-key vault (cli-common
//! `pk_cli_auth::vault`), so the whole CLI reads one keychain item.
//!
//! ICANotes shows the secret once and takes both in the query string, so
//! neither is ever printed, logged, put in a plain file or taken from argv,
//! and every text that could carry a request URL is redacted
//! ([`crate::redact`]).

use pk_cli_auth::vault::Vault;
use pk_cli_core::CliError;
use pk_cli_secrets::Secret;
use serde::{Deserialize, Serialize};

use crate::redact::Redactor;

/// The entry's shape. A release refuses an entry with a newer one rather
/// than read it in its own shape and write it back.
pub const FORMAT: u32 = 1;

/// The vault entry for a profile's patient at an organization.
pub fn entry(profile: &str, organization: &str, patient: &str) -> String {
    format!(
        "{}/{}/{}",
        encode(profile),
        encode(organization),
        encode(patient)
    )
}

fn encode(part: &str) -> String {
    part.replace('%', "%25").replace('/', "%2F")
}

/// The key and secret. No `Debug` that shows either.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credential {
    pub format: u32,
    pub user_key: String,
    pub user_secret: String,
    /// The id ICANotes' `PatientID` answered for this key.
    pub patient_id: String,
    pub stored_at: String,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("format", &self.format)
            .field("patient_id", &"…")
            .finish_non_exhaustive()
    }
}

/// A key and secret not yet tied to a patient.
#[derive(Clone)]
pub struct KeyPair {
    pub user_key: String,
    pub user_secret: String,
}

impl std::fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeyPair(..)")
    }
}

const SHAPE: &str = "the API key and secret are the single-line values the portal's FHIR API Access page shows, with no spaces";

impl KeyPair {
    /// Check a key and secret without quoting either in an error.
    pub fn new(key: &str, secret: &str) -> Result<Self, CliError> {
        let ok =
            |v: &str| !v.is_empty() && v.len() <= 1000 && v.bytes().all(|b| b.is_ascii_graphic());
        let key = key.trim();
        let secret = secret.trim();
        if !ok(key) || !ok(secret) {
            return Err(CliError::Usage(SHAPE.into()));
        }
        Ok(KeyPair {
            user_key: key.into(),
            user_secret: secret.into(),
        })
    }

    /// `<key>:<secret>`, or the key and the secret on two lines, as
    /// `--stdin` and `--from-env` take them. A key holds no colon; a secret
    /// may.
    pub fn parse(text: &Secret) -> Result<Self, CliError> {
        let text = text.expose().trim();
        let split = text
            .split_once('\n')
            .or_else(|| text.split_once(':'))
            .ok_or_else(|| {
                CliError::Usage(
                    "give the API key and secret as `<key>:<secret>`, or on two lines".into(),
                )
            })?;
        Self::new(split.0, split.1)
    }

    pub fn redactor(&self) -> Redactor {
        Redactor::new([self.user_key.as_str(), self.user_secret.as_str()])
    }

    pub fn into_credential(self, patient_id: String, stored_at: String) -> Credential {
        Credential {
            format: FORMAT,
            user_key: self.user_key,
            user_secret: self.user_secret,
            patient_id,
            stored_at,
        }
    }
}

impl Credential {
    pub fn pair(&self) -> KeyPair {
        KeyPair {
            user_key: self.user_key.clone(),
            user_secret: self.user_secret.clone(),
        }
    }

    pub fn redactor(&self) -> Redactor {
        Redactor::new([self.user_key.as_str(), self.user_secret.as_str()])
    }
}

pub fn read(vault: &Vault, name: &str) -> Result<Option<Credential>, CliError> {
    let c: Option<Credential> = vault.get_json(name)?;
    if let Some(c) = &c {
        if c.format > FORMAT {
            return Err(CliError::Auth(
                "the stored API key was written by a newer release of icanotes".into(),
            )
            .because(pk_cli_core::Reason::CredentialUnreadable)
            .with_remedy("run `icanotes self-update`"));
        }
    }
    Ok(c)
}

pub fn store(vault: &Vault, name: &str, credential: &Credential) -> Result<(), CliError> {
    vault.put_json(name, credential)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_reads_in_either_form() {
        let a = KeyPair::parse(&Secret::new("key-one:secret-one\n")).unwrap();
        assert_eq!(
            (a.user_key.as_str(), a.user_secret.as_str()),
            ("key-one", "secret-one")
        );
        let b = KeyPair::parse(&Secret::new("key-two\nsec:ret\n")).unwrap();
        assert_eq!(
            (b.user_key.as_str(), b.user_secret.as_str()),
            ("key-two", "sec:ret")
        );
    }

    #[test]
    fn a_bad_pair_is_a_usage_error_that_quotes_nothing() {
        for bad in [
            "only-one-part",
            "has space:secret-x",
            ":secret-x",
            "key-x:",
            "key-x\n",
        ] {
            let err = KeyPair::parse(&Secret::new(bad)).err().unwrap();
            assert_eq!(err.exit_code(), 2, "{bad}");
            let m = err.to_string();
            assert!(
                !m.contains("secret-x") && !m.contains("key-x") && !m.contains("has space"),
                "{m}"
            );
        }
    }

    #[test]
    fn debug_shows_neither_value() {
        let c = KeyPair::new("key-one", "secret-one")
            .unwrap()
            .into_credential("1001".into(), "2020-01-01T00:00:00Z".into());
        let d = format!("{c:?} {:?}", c.pair());
        assert!(
            !d.contains("key-one") && !d.contains("secret-one") && !d.contains("1001"),
            "{d}"
        );
    }

    #[test]
    fn entries_cannot_collide() {
        assert_eq!(entry("adult1", "icanotes", "1001"), "adult1/icanotes/1001");
        assert_ne!(entry("a", "b/c", "d"), entry("a", "b", "c/d"));
    }

    #[test]
    fn the_credential_round_trips_and_a_newer_format_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::new(
            pk_cli_auth::vault::MemoryKeys::new(),
            dir.path().join("vault"),
        );
        let name = entry("adult1", "icanotes", "1001");
        assert!(read(&vault, &name).unwrap().is_none());
        let c = KeyPair::new("key-one", "secret-one")
            .unwrap()
            .into_credential("1001".into(), "2020-01-01T00:00:00Z".into());
        store(&vault, &name, &c).unwrap();
        assert_eq!(read(&vault, &name).unwrap().unwrap().user_key, "key-one");
        let mut newer = c.clone();
        newer.format = FORMAT + 1;
        store(&vault, &name, &newer).unwrap();
        assert_eq!(read(&vault, &name).unwrap_err().exit_code(), 3);
    }
}
