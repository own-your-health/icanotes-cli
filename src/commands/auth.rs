//! `icanotes auth login|set-credential|status|logout`.

use std::sync::Arc;

use pk_cli_auth::{AuthLogin, AuthMethod, AuthStatus};
use pk_cli_browser::Confirm;
use pk_cli_config::profile::ProfileName;
use pk_cli_core::{output, CliError};
use serde_json::{json, Value};

use super::{minutes, Ctx, BIN};
use crate::cli::{AuthCmd, LoginArgs, LogoutArgs, SetCredentialArgs, StatusArgs};
use crate::config::{Grant, ORG_ID, PORTAL_HOST};
use crate::credential::{self, KeyPair};
use crate::login::{Verdict, Verdicts, WindowRequest};

pub fn run(ctx: &Ctx, cmd: &AuthCmd) -> Result<(), CliError> {
    match cmd {
        AuthCmd::Login(args) => login(ctx, args),
        AuthCmd::SetCredential(args) => set_credential(ctx, args),
        AuthCmd::Status(args) => status(ctx, args),
        AuthCmd::Logout(args) => logout(ctx, args),
    }
}

/// A household label: 1 to 32 of `a-z 0-9 - _`, as profile names are.
pub fn check_label(label: Option<&str>) -> Result<(), CliError> {
    if let Some(l) = label {
        let ok = !l.is_empty()
            && l.len() <= 32
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        if !ok {
            return Err(CliError::Usage(
                "a --label is 1 to 32 lowercase letters, digits, `-` or `_` (`adult1`)".into(),
            ));
        }
    }
    Ok(())
}

fn login(ctx: &Ctx, args: &LoginArgs) -> Result<(), CliError> {
    if !args.browser {
        return Err(CliError::Usage(
            "`auth login` opens the patient portal in a browser window: pass --browser".into(),
        )
        .with_remedy(format!(
            "run `{BIN} auth login --browser --profile <NAME>`, or store a key generated on the portal with `{BIN} auth set-credential --profile <NAME>`"
        )));
    }
    check_label(args.label.as_deref())?;
    if args.timeout == 0 {
        return Err(CliError::Usage("--timeout is at least 1 minute".into()));
    }
    let config = ctx.config()?;
    let profile = ctx.profile(&config)?;
    let url = config.portal_url();
    let host = url::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| PORTAL_HOST.into());
    // The check inside the window may wait for a request window, but not
    // for long: the page holds a secret it shows once.
    let api = Arc::new(ctx.api(&config, minutes(5))?);
    let verdicts = Arc::new(Verdicts::default());
    let confirm = {
        let api = api.clone();
        let verdicts = verdicts.clone();
        Arc::new(move |pair: &KeyPair| -> Confirm {
            match verdicts.get(pair) {
                Some(Verdict::Works(_)) => return Confirm::Ready,
                Some(Verdict::Refused(why)) => return Confirm::NotYet(why),
                None => {}
            }
            match api.patient_id(pair) {
                Ok(id) => {
                    verdicts.put(pair, Verdict::Works(id));
                    Confirm::Ready
                }
                Err(e) if e.exit_code() == 3 => {
                    let why = "ICANotes refused the key on the page (PatientID)".to_string();
                    verdicts.put(pair, Verdict::Refused(why.clone()));
                    Confirm::NotYet(why)
                }
                Err(e) => Confirm::undecided(e.message().to_string()),
            }
        })
    };
    ctx.note(format!(
        "Opening the patient portal for profile {profile}. Sign in, open My History, then FHIR API Access, and choose Generate New Credentials. icanotes reads the key off the page and closes the window."
    ));
    let pair = (ctx.window)(&WindowRequest {
        profile: &profile,
        url: &url,
        host: &host,
        timeout: minutes(args.timeout),
        confirm,
    })?;
    // The window hands over only a key the check accepted, so it has a
    // patient id; ask once more only if the window was a stand-in.
    let patient_id = match verdicts.accepted(&pair) {
        Some(id) => id,
        None => api.patient_id(&pair)?,
    };
    let grant = store(
        ctx,
        &profile,
        args.label.clone(),
        pair,
        patient_id,
        "browser",
    )?;
    report_stored(ctx, &profile, &grant, true);
    Ok(())
}

fn set_credential(ctx: &Ctx, args: &SetCredentialArgs) -> Result<(), CliError> {
    check_label(args.label.as_deref())?;
    if let Some(id) = &args.patient_id {
        if !pk_med_fhir::is_id(id) {
            return Err(CliError::Usage(
                "--patient-id is 1 to 256 of A-Z a-z 0-9 - .".into(),
            ));
        }
    }
    let config = ctx.config()?;
    let profile = ctx.profile(&config)?;
    let secret = args
        .source
        .read(Some("ICANotes API key and secret, as <key>:<secret>"))?;
    let pair = KeyPair::parse(&secret)?;
    let (patient_id, verified) = if args.no_verify {
        (args.patient_id.clone().unwrap_or_default(), false)
    } else {
        let api = ctx.api(&config, minutes(16))?;
        let id = api.patient_id(&pair)?;
        if let Some(given) = &args.patient_id {
            if *given != id {
                return Err(CliError::Usage(
                    "ICANotes ties this key to another patient id than --patient-id".into(),
                )
                .with_remedy("leave out --patient-id; ICANotes gives it"));
            }
        }
        (id, true)
    };
    let grant = store(ctx, &profile, args.label.clone(), pair, patient_id, "stdin")?;
    report_stored(ctx, &profile, &grant, verified);
    Ok(())
}

/// The key into the vault, then the grant into the config, so a grant the
/// config lists always has its key.
fn store(
    ctx: &Ctx,
    profile: &ProfileName,
    label: Option<String>,
    pair: KeyPair,
    patient_id: String,
    method: &str,
) -> Result<Grant, CliError> {
    let config = ctx.config()?;
    let mut settings = ctx.settings(&config, profile)?;
    // A label stays the household's: one already given keeps it unless a
    // new one is passed.
    let label = label.or_else(|| {
        settings
            .grants
            .iter()
            .find(|g| g.id == patient_id && g.organization == ORG_ID)
            .and_then(|g| g.label.clone())
    });
    let stored_at = pk_cli_core::dates::fmt_rfc3339(crate::ratelimit::now());
    let grant = Grant {
        id: patient_id.clone(),
        organization: ORG_ID.into(),
        label,
        stored_at: stored_at.clone(),
        method: method.into(),
    };
    let cred = pair.into_credential(patient_id, stored_at);
    credential::store(&ctx.vault, &grant.entry(profile.as_str()), &cred)?;
    settings.upsert(grant.clone());
    ctx.save_settings(profile, &settings)?;
    Ok(grant)
}

fn report_stored(ctx: &Ctx, profile: &ProfileName, grant: &Grant, verified: bool) {
    let mut v = AuthLogin::stored(verified).to_json();
    v["profile"] = profile.as_str().into();
    v["patient"] = serde_json::to_value(grant.context(profile.as_str())).unwrap_or_default();
    if ctx.json() {
        output::json(&v);
    } else {
        // The label, not the patient id: `patients list` shows the id to
        // whoever needs it, and this line lands in transcripts.
        let who = grant
            .label
            .as_deref()
            .map(|l| format!("patient {l}"))
            .unwrap_or_else(|| "the patient".into());
        println!(
            "Stored the API key for {who} in profile {profile}{}.",
            if verified {
                ", checked with ICANotes"
            } else {
                " (not checked)"
            }
        );
        println!("Export with `{BIN} records export --profile {profile} --ndjson`.");
    }
}

pub fn status_of(ctx: &Ctx, check: bool) -> Result<(AuthStatus, Value), CliError> {
    let config = ctx.config()?;
    let profile = ctx.profile(&config)?;
    let settings = ctx.settings(&config, &profile)?;
    let api = if check && !settings.grants.is_empty() {
        Some(ctx.api(&config, minutes(16))?)
    } else {
        None
    };
    let mut rows = Vec::new();
    let mut any_stored = false;
    let mut any_works = false;
    for g in &settings.grants {
        let mut row = serde_json::to_value(g.context(profile.as_str())).unwrap_or_default();
        row["stored_at"] = g.stored_at.clone().into();
        row["method"] = g.method.clone().into();
        // The vault reads its key only when the entry exists, so a profile
        // with nothing stored raises no keychain prompt.
        let cred = match credential::read(&ctx.vault, &g.entry(profile.as_str())) {
            Ok(c) => c,
            Err(e) => {
                row["error"] = e.message().into();
                None
            }
        };
        row["credential_in_keychain"] = cred.is_some().into();
        any_stored |= cred.is_some();
        if let (Some(api), Some(cred)) = (&api, &cred) {
            match api.patient_id(&cred.pair()) {
                Ok(id) if id == cred.patient_id => {
                    row["works"] = true.into();
                    any_works = true;
                }
                Ok(_) => {
                    row["works"] = false.into();
                    row["error"] = "ICANotes ties the key to another patient now".into();
                }
                Err(e) => {
                    row["works"] = false.into();
                    row["error"] = e.message().into();
                }
            }
        }
        rows.push(row);
    }
    let usable = if check { any_works } else { any_stored };
    let mut status = AuthStatus::new(true, usable, AuthMethod::Password);
    status.credential_in_keychain = Some(any_stored);
    if check {
        status.session_valid = Some(any_works);
    }
    status.browser_profile = pk_cli_browser::named_profile_exists(BIN, profile.as_str()).ok();
    let mut v = status.to_json();
    v["profile"] = profile.as_str().into();
    v["patients"] = Value::Array(rows);
    Ok((status, v))
}

fn status(ctx: &Ctx, args: &StatusArgs) -> Result<(), CliError> {
    let (status, v) = status_of(ctx, args.check)?;
    if ctx.json() {
        output::json(&v);
        return Ok(());
    }
    let profile = v["profile"].as_str().unwrap_or_default();
    println!("Profile:       {profile}");
    status.render();
    let rows = v["patients"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        println!("Patients:      none (`{BIN} auth login --browser --profile {profile}`)");
    }
    for r in rows {
        let who = match r["label"].as_str() {
            Some(l) => format!("{l} ({})", r["id"].as_str().unwrap_or_default()),
            None => r["id"].as_str().unwrap_or_default().to_string(),
        };
        let state = if let Some(e) = r["error"].as_str() {
            format!("not usable: {e}")
        } else if r["works"] == true {
            "key stored, ICANotes accepts it".into()
        } else if r["credential_in_keychain"] == true {
            "key stored".into()
        } else {
            "no key stored".into()
        };
        println!("Patient {who}: {state}");
    }
    Ok(())
}

fn logout(ctx: &Ctx, args: &LogoutArgs) -> Result<(), CliError> {
    let config = ctx.config()?;
    let profile = ctx.profile(&config)?;
    let mut settings = ctx.settings(&config, &profile)?;
    let chosen: Vec<Grant> = match args.patient.as_deref() {
        Some(_) => vec![settings
            .pick(args.patient.as_deref(), profile.as_str())?
            .clone()],
        None => settings.grants.clone(),
    };
    let mut removed = 0;
    for g in &chosen {
        if ctx.vault.delete(&g.entry(profile.as_str()))? {
            removed += 1;
        }
        settings.remove(&g.organization, &g.id);
    }
    ctx.save_settings(&profile, &settings)?;
    // With nothing left in the vault, its key item goes too, so icanotes
    // leaves nothing in the Keychain. No vault directory means no key was
    // ever made, and the Keychain is not touched.
    let mut cleared = false;
    let mut kept = 0;
    if args.forget && ctx.vault.dir().exists() {
        let left = ctx.vault.list()?;
        kept = left.names.len() + left.unreadable.len();
        if kept == 0 {
            ctx.vault.clear()?;
            cleared = true;
        }
    }
    let v = json!({
        "profile": profile.as_str(),
        "patients_removed": chosen.len(),
        "keys_removed": removed,
        "vault_cleared": cleared,
        "keys_kept_elsewhere": kept,
        "note": "a key stays valid at ICANotes until it is deleted on the portal's FHIR API Access page",
    });
    output::emit(ctx.json(), "auth-logout", v, |_| {
        println!(
            "Removed {removed} stored key(s) from profile {profile}. They stay valid at ICANotes until deleted on the portal's FHIR API Access page."
        );
        if cleared {
            println!("The vault is empty, and its Keychain key is deleted.");
        } else if args.forget && kept > 0 {
            println!(
                "The vault still holds {kept} key(s) of other profiles, so its Keychain key stays."
            );
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_checked() {
        check_label(None).unwrap();
        check_label(Some("adult1")).unwrap();
        for bad in ["", "Adult1", "a b", &"x".repeat(33)] {
            assert_eq!(check_label(Some(bad)).unwrap_err().exit_code(), 2, "{bad}");
        }
    }
}
