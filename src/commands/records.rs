//! `icanotes records export`: records-stream/v1 to stdout (`--ndjson`) or to
//! `DIR/records.ndjson` (`-o`).

use pk_cli_core::{output, CliError};
use pk_med_records::{create_export_file, RecordsExport};

use super::{minutes, report, Ctx, VERSION};
use crate::cli::{ExportArgs, RecordsCmd};
use crate::config::{is_test_server, organization};
use crate::export::{self, Plan};

pub fn run(ctx: &Ctx, cmd: &RecordsCmd) -> Result<(), CliError> {
    match cmd {
        RecordsCmd::Export(args) => export_cmd(ctx, args),
    }
}

/// `--since` as a day, refusing one after today.
pub fn parse_since(since: Option<&str>) -> Result<Option<pk_cli_core::dates::Civil>, CliError> {
    let Some(s) = since else { return Ok(None) };
    let day = pk_cli_core::dates::parse_iso(s)
        .map_err(|_| CliError::Usage(format!("--since `{s}` is not a YYYY-MM-DD date")))?;
    if day > pk_cli_core::dates::today() {
        return Err(CliError::Usage(format!("--since `{s}` is after today")));
    }
    Ok(Some(day))
}

fn export_cmd(ctx: &Ctx, args: &ExportArgs) -> Result<(), CliError> {
    // Arguments first: nothing here touches the keychain or the network.
    if !args.ndjson && args.output.is_none() {
        return Err(CliError::Usage(
            "say where the stream goes: --ndjson (stdout) or -o <DIR>".into(),
        ));
    }
    if args.ndjson && ctx.json() {
        return Err(CliError::Usage(
            "--ndjson writes the stream itself to stdout; leave out --json".into(),
        ));
    }
    let since = parse_since(args.since.as_deref())?;
    let config = ctx.config()?;
    if args.output.is_some() && !is_test_server(&config.api_base()) {
        return Err(CliError::Usage(
            "-o writes a test server's synthetic records only; a patient's own go to stdout".into(),
        )
        .with_remedy(
            "pass --ndjson, which `medrec import --from icanotes` reads, and let medrec file them",
        ));
    }
    let profile = ctx.profile(&config)?;
    let (grant, cred) = ctx.grant(&config, &profile, &args.who)?;
    let api = ctx.api(&config, minutes(args.who.max_wait))?;
    let plan = Plan {
        credential: &cred,
        patient: grant.context(profile.as_str()),
        organization: organization(&config.api_base()),
        since,
        today: pk_cli_core::dates::today(),
        pulled_at: pk_cli_core::dates::fmt_rfc3339(crate::ratelimit::now()),
        version: VERSION,
    };
    let note = |m: &str| ctx.note(m);
    match &args.output {
        None => {
            let stdout = std::io::stdout();
            let (_, ended) = export::run(&api, &plan, stdout.lock(), &note)?;
            if let Some(e) = ended {
                // The stream is the output: the error goes to stderr only.
                report(&e);
                std::process::exit(e.exit_code());
            }
            Ok(())
        }
        Some(dir) => {
            let mut file = create_export_file(dir)?;
            let (finished, ended) = export::run(&api, &plan, file.file(), &note)?;
            let path = file.commit(&finished)?;
            let dto = RecordsExport::new(path.display().to_string(), &finished);
            // records-export/v1 carries its own schema.
            let v = serde_json::to_value(&dto).unwrap_or_default();
            if ctx.json() {
                output::json(&v);
            } else {
                output::render(&v);
            }
            if let Some(e) = ended {
                report(&e);
                std::process::exit(e.exit_code());
            }
            Ok(())
        }
    }
}
