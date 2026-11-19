//! `icanotes api GET <Endpoint>?<query>`: one request to the patient API as
//! the profile's patient. The key, secret and patient id are added; the
//! answer is printed (or written with -o) with the key and secret redacted.

use std::io::Write;

use pk_cli_core::private_file::{self, Replace};
use pk_cli_core::CliError;

use super::{minutes, Ctx};
use crate::cli::ApiArgs;
use crate::client::{check_endpoint, status_error, RESERVED};

/// The endpoint and its query pairs, refusing what icanotes sets itself.
pub fn parse_path(path: &str) -> Result<(String, Vec<(String, String)>), CliError> {
    let path = path.trim().trim_start_matches('/');
    let path = path.strip_prefix("API/").unwrap_or(path);
    let (endpoint, query) = path.split_once('?').unwrap_or((path, ""));
    check_endpoint(endpoint)?;
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if let Some((k, _)) = pairs
        .iter()
        .find(|(k, _)| RESERVED.iter().any(|r| r.eq_ignore_ascii_case(k)))
    {
        return Err(CliError::Usage(format!(
            "leave `{k}` out: icanotes adds the stored key and secret itself"
        )));
    }
    Ok((endpoint.to_string(), pairs))
}

pub fn run(ctx: &Ctx, args: &ApiArgs) -> Result<(), CliError> {
    if !args.method.eq_ignore_ascii_case("GET") {
        return Err(CliError::Usage(format!(
            "`{}`: the patient API reads, so `api` sends GET only",
            args.method
        )));
    }
    let (endpoint, mut pairs) = parse_path(&args.path)?;
    let config = ctx.config()?;
    let profile = ctx.profile(&config)?;
    let (_, cred) = ctx.grant(&config, &profile, &args.who)?;
    let wants_patient = endpoint != "PatientID";
    if wants_patient
        && !pairs
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("patientID"))
    {
        pairs.insert(0, ("patientID".into(), cred.patient_id.clone()));
    }
    let api = ctx.api(&config, minutes(args.who.max_wait))?;
    let params: Vec<(&str, String)> = pairs.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let resp = api.send(&endpoint, &cred.pair(), &params)?;
    let redactor = cred.redactor();
    let body = redactor.bytes(&resp.body);
    if let Some(path) = &args.output {
        private_file::write(path, &body, Replace::Always)
            .map_err(|e| CliError::Other(format!("writing {}: {e}", path.display())))?;
        ctx.note(format!(
            "HTTP {}: {} bytes to {}",
            resp.status,
            body.len(),
            path.display()
        ));
    } else {
        let text = std::str::from_utf8(&body).map_err(|_| {
            CliError::Usage(format!(
                "{endpoint} answered {} bytes that are not text (a ZIP, for PatientCCD)",
                body.len()
            ))
            .with_remedy("write them to a file with -o <FILE>")
        })?;
        let shown = match serde_json::from_str::<serde_json::Value>(text) {
            // Pretty only for display; `-o` keeps the server's own bytes.
            // Re-serializing turns escapes back into characters, so the
            // result is redacted again.
            Ok(v) if ctx.json() || std::io::IsTerminal::is_terminal(&std::io::stdout()) => redactor
                .text(&serde_json::to_string_pretty(&v).unwrap_or_else(|_| text.to_string())),
            _ => text.to_string(),
        };
        let mut out = std::io::stdout().lock();
        writeln!(out, "{shown}").map_err(|e| CliError::Other(format!("writing stdout: {e}")))?;
    }
    if !(200..300).contains(&resp.status) {
        let e = status_error(resp.status, &endpoint);
        super::report(&e);
        std::process::exit(e.exit_code());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_splits_into_endpoint_and_query() {
        let (e, q) = parse_path("/API/Condition?req_type=Complete&cat_type=All").unwrap();
        assert_eq!(e, "Condition");
        assert_eq!(
            q,
            vec![
                ("req_type".into(), "Complete".into()),
                ("cat_type".into(), "All".into())
            ]
        );
        assert_eq!(parse_path("PatientID").unwrap().0, "PatientID");
    }

    #[test]
    fn the_key_is_never_taken_from_the_path() {
        for bad in ["Goal?userKey=x", "Goal?usersecret=y", "../Goal", "Goal/1"] {
            assert_eq!(parse_path(bad).unwrap_err().exit_code(), 2, "{bad}");
        }
    }
}
