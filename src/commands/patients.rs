//! `icanotes patients list`: the patients whose keys a profile holds. Local:
//! no request, no keychain.

use pk_cli_core::{output, CliError};
use pk_med_records::patient_list;

use super::{Ctx, BIN};
use crate::cli::PatientsCmd;

pub fn run(ctx: &Ctx, cmd: &PatientsCmd) -> Result<(), CliError> {
    match cmd {
        PatientsCmd::List => list(ctx),
    }
}

fn list(ctx: &Ctx) -> Result<(), CliError> {
    let config = ctx.config()?;
    let profile = ctx.profile(&config)?;
    let settings = ctx.settings(&config, &profile)?;
    let items: Vec<_> = settings
        .grants
        .iter()
        .map(|g| g.context(profile.as_str()))
        .collect();
    if ctx.json() {
        output::json(
            &serde_json::to_value(patient_list(items)).expect("patient-list/v1 serializes"),
        );
    } else if items.is_empty() {
        println!(
            "Profile {profile} holds no key (`{BIN} auth login --browser --profile {profile}`)."
        );
    } else {
        for c in &items {
            match &c.label {
                Some(l) => println!("{}  {}  {l}", c.id, c.organization),
                None => println!("{}  {}", c.id, c.organization),
            }
        }
    }
    Ok(())
}
