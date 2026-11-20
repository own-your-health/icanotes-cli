//! Non-secret settings (`~/.config/icanotes/config.json`): the API's base for
//! a test server, and the profiles, one per patient-portal login, each with
//! the patients whose API keys it holds. The keys and secrets themselves live
//! in icanotes' vault, never here.

use pk_cli_config::profile::Profiles;
use pk_cli_core::CliError;
use pk_med_records::{Organization, PatientContext};
use serde::{Deserialize, Serialize};

/// The one organization: ICANotes' patient API, which answers for every
/// practice on ICANotes. A key names its patient, and so its practice.
pub const ORG_ID: &str = "icanotes";
pub const ORG_NAME: &str = "ICANotes patient API";
pub const VENDOR: &str = "icanotes";
/// The production API. `r2test.icanotes.com` is ICANotes' test server, with
/// keys of its own.
pub const API_BASE: &str = "https://r2.icanotes.com/API";
/// The patient portal, where a patient signs in and generates a key.
pub const PORTAL_URL: &str = "https://patientonlineportal.com/";
pub const PORTAL_HOST: &str = "patientonlineportal.com";

pub const KEYS: &str = "api_base, portal_url";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Another API base: a test server on this machine
    /// (`http://127.0.0.1:<port>/API`), or ICANotes' own on another host of
    /// icanotes.com.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_base: Option<String>,
    /// Another portal address for `auth login --browser`: a test page on this
    /// machine, or the portal at another https address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portal_url: Option<String>,
    /// One per portal login (`adult1`), each with its [`ProfileSettings`].
    #[serde(default, skip_serializing_if = "Profiles::is_empty")]
    pub profiles: Profiles,
}

/// A profile's settings: the patients whose keys its login generated. Their
/// keys and secrets are in the vault, as `<profile>/icanotes/<patient id>`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProfileSettings {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<Grant>,
}

/// One patient a profile holds a key for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// The patient id ICANotes' `PatientID` answers for the key.
    pub id: String,
    pub organization: String,
    /// The household's own name for the patient (`--label`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// When the key was stored, RFC 3339.
    pub stored_at: String,
    /// How it was stored: `browser` (read off the portal's page) or `stdin`.
    pub method: String,
}

impl Grant {
    /// The vault entry holding this grant's key and secret.
    pub fn entry(&self, profile: &str) -> String {
        crate::credential::entry(profile, &self.organization, &self.id)
    }

    pub fn context(&self, profile: &str) -> PatientContext {
        PatientContext {
            id: self.id.clone(),
            organization: self.organization.clone(),
            label: self.label.clone(),
            relationship: Some(pk_med_records::dto::relationship::SELF.into()),
            kind: Some(pk_med_records::dto::kind::PATIENT.into()),
            member_role: None,
            fhir_user: None,
            name: None,
            session: Some(self.entry(profile)),
        }
    }
}

impl ProfileSettings {
    /// Insert or replace the grant for the same (organization, patient).
    pub fn upsert(&mut self, grant: Grant) {
        self.remove(&grant.organization, &grant.id);
        self.grants.push(grant);
    }

    pub fn remove(&mut self, organization: &str, id: &str) -> bool {
        let before = self.grants.len();
        self.grants
            .retain(|g| !(g.organization == organization && g.id == id));
        before != self.grants.len()
    }

    /// The grant `who` names: a label or a patient id; with none named, the
    /// only one.
    pub fn pick(&self, who: Option<&str>, profile: &str) -> Result<&Grant, CliError> {
        let bin = crate::commands::BIN;
        match who {
            Some(w) => {
                let by_label: Vec<&Grant> = self
                    .grants
                    .iter()
                    .filter(|g| g.label.as_deref() == Some(w))
                    .collect();
                if by_label.len() == 1 {
                    return Ok(by_label[0]);
                }
                if let Some(g) = self.grants.iter().find(|g| g.id == w) {
                    return Ok(g);
                }
                if by_label.len() > 1 {
                    return Err(CliError::Usage(format!(
                        "more than one patient in profile {profile} is labelled `{w}`"
                    ))
                    .with_remedy(format!(
                        "pass the patient id from `{bin} patients list --profile {profile}`"
                    )));
                }
                Err(CliError::Usage(format!(
                    "profile {profile} holds no patient `{w}`"
                ))
                .with_remedy(format!("`{bin} patients list --profile {profile}` shows who it holds")))
            }
            None => match self.grants.as_slice() {
                [only] => Ok(only),
                [] => Err(CliError::Auth(format!(
                    "profile {profile} holds no ICANotes API key"
                ))
                .because(pk_cli_core::Reason::NoCredential)
                .with_remedy(format!(
                    "run `{bin} auth login --browser --profile {profile}`, or store a key with `{bin} auth set-credential --profile {profile}`; {} explains what the practice and the patient do first",
                    crate::commands::SETUP_DOC
                ))),
                _ => Err(CliError::Usage(format!(
                    "profile {profile} holds more than one patient; say which with --patient"
                ))
                .with_remedy(format!("`{bin} patients list --profile {profile}` shows them"))),
            },
        }
    }
}

pub fn organization(api_base: &str) -> Organization {
    Organization {
        id: ORG_ID.into(),
        name: ORG_NAME.into(),
        fhir_base: pk_med_fhir::normalize_base(api_base),
        vendor: Some(VENDOR.into()),
        location: None,
        aliases: vec!["patientonlineportal.com".into(), "ICANotes".into()],
    }
}

impl Config {
    pub fn api_base(&self) -> String {
        pk_med_fhir::normalize_base(self.api_base.as_deref().unwrap_or(API_BASE))
    }

    pub fn portal_url(&self) -> String {
        self.portal_url.clone().unwrap_or_else(|| PORTAL_URL.into())
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<(), CliError> {
        match key {
            "api_base" => {
                check_api_base(value)?;
                self.api_base = Some(pk_med_fhir::normalize_base(value));
            }
            "portal_url" => {
                check_portal_url(value)?;
                self.portal_url = Some(value.to_string());
            }
            other => return Err(unknown(other)),
        }
        Ok(())
    }

    /// The stored `api_base` and `portal_url` checked as `set` checks them,
    /// for a config written by hand or named by `--config`: the stored key
    /// goes to `api_base`, so a host anywhere else is refused before the
    /// vault is read.
    pub fn check(&self) -> Result<(), CliError> {
        let fix = |key: &str| {
            format!("`icanotes config unset {key}` returns to ICANotes' own, or `icanotes config set {key} <URL>`")
        };
        if let Some(v) = &self.api_base {
            check_api_base(v).map_err(|e| {
                CliError::Usage(format!("the config's api_base: {}", e.message()))
                    .with_remedy(fix("api_base"))
            })?;
        }
        if let Some(v) = &self.portal_url {
            check_portal_url(v).map_err(|e| {
                CliError::Usage(format!("the config's portal_url: {}", e.message()))
                    .with_remedy(fix("portal_url"))
            })?;
        }
        Ok(())
    }

    pub fn unset(&mut self, key: &str) -> Result<(), CliError> {
        match key {
            "api_base" => self.api_base = None,
            "portal_url" => self.portal_url = None,
            other => return Err(unknown(other)),
        }
        Ok(())
    }
}

fn check_api_base(value: &str) -> Result<(), CliError> {
    pk_cli_core::transport::require_secure_url(value)?;
    if !(is_icanotes(value) || is_loopback(value)) {
        return Err(CliError::Usage(
            "api_base is ICANotes' (https on icanotes.com) or a test server on this machine; the API key goes nowhere else".into(),
        ));
    }
    Ok(())
}

fn check_portal_url(value: &str) -> Result<(), CliError> {
    pk_cli_core::transport::require_secure_url(value)?;
    url::Url::parse(value).map_err(|_| CliError::Usage(format!("`{value}` is not a URL")))?;
    Ok(())
}

fn unknown(key: &str) -> CliError {
    CliError::Usage(format!("unknown config key `{key}` (known: {KEYS})"))
}

fn host(value: &str) -> Option<String> {
    url::Url::parse(value)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
}

/// https on icanotes.com or a host under it.
pub fn is_icanotes(value: &str) -> bool {
    value.starts_with("https://")
        && host(value).is_some_and(|h| h == "icanotes.com" || h.ends_with(".icanotes.com"))
}

/// http or https to this machine: a test server.
pub fn is_loopback(value: &str) -> bool {
    host(value).is_some_and(|h| h == "127.0.0.1" || h == "localhost" || h == "[::1]")
}

/// ICANotes' test host, which serves its published synthetic patient.
pub const TEST_HOST: &str = "r2test.icanotes.com";

/// A server whose records are synthetic: a mock on this machine, or
/// ICANotes' test host. Only these may be exported to a file.
pub fn is_test_server(api_base: &str) -> bool {
    is_loopback(api_base) || host(api_base).is_some_and(|h| h == TEST_HOST)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(id: &str, label: Option<&str>) -> Grant {
        Grant {
            id: id.into(),
            organization: ORG_ID.into(),
            label: label.map(str::to_string),
            stored_at: "2020-01-01T00:00:00Z".into(),
            method: "stdin".into(),
        }
    }

    #[test]
    fn defaults() {
        let c = Config::default();
        assert_eq!(c.api_base(), API_BASE);
        assert_eq!(c.portal_url(), PORTAL_URL);
    }

    #[test]
    fn api_base_goes_only_to_icanotes_or_this_machine() {
        let mut c = Config::default();
        assert!(c.set("api_base", "http://r2.icanotes.com/API").is_err());
        assert!(c.set("api_base", "https://example.com/API").is_err());
        assert!(c
            .set("api_base", "https://icanotes.com.example.com/API")
            .is_err());
        c.set("api_base", "https://r2test.icanotes.com/API/")
            .unwrap();
        assert_eq!(c.api_base(), "https://r2test.icanotes.com/API");
        c.set("api_base", "http://127.0.0.1:9/API").unwrap();
        assert_eq!(c.api_base(), "http://127.0.0.1:9/API");
        c.unset("api_base").unwrap();
        assert_eq!(c.api_base(), API_BASE);
        // A config written by hand is held to the same rule.
        c.api_base = Some("http://example.net/API".into());
        let e = c.check().unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert!(e.remedy().unwrap().contains("config unset api_base"));
        c.api_base = Some("https://r2.icanotes.com/API".into());
        c.portal_url = Some("http://patientonlineportal.com/".into());
        assert_eq!(c.check().unwrap_err().exit_code(), 2);
        c.portal_url = None;
        c.check().unwrap();
        assert_eq!(c.set("nope", "x").unwrap_err().exit_code(), 2);
    }

    #[test]
    fn pick_by_label_id_or_the_only_one() {
        let mut s = ProfileSettings::default();
        assert_eq!(s.pick(None, "p").unwrap_err().exit_code(), 3);
        s.upsert(grant("1001", Some("adult1")));
        assert_eq!(s.pick(None, "p").unwrap().id, "1001");
        s.upsert(grant("1002", None));
        assert_eq!(s.pick(None, "p").unwrap_err().exit_code(), 2);
        assert_eq!(s.pick(Some("adult1"), "p").unwrap().id, "1001");
        assert_eq!(s.pick(Some("1002"), "p").unwrap().id, "1002");
        assert_eq!(s.pick(Some("nobody"), "p").unwrap_err().exit_code(), 2);
        s.upsert(grant("1002", Some("adult2")));
        assert_eq!(s.grants.len(), 2);
        assert!(s.remove(ORG_ID, "1001"));
        assert!(!s.remove(ORG_ID, "1001"));
    }

    #[test]
    fn a_grant_is_a_patient_context_naming_its_vault_entry() {
        let c = grant("1001", Some("adult1")).context("adult1");
        assert_eq!(c.session.as_deref(), Some("adult1/icanotes/1001"));
        assert_eq!(c.kind.as_deref(), Some("patient"));
    }
}
