//! `icanotes profiles list|add|remove`: the logins icanotes keeps, one per
//! patient-portal account. Names are the household's own (`adult1`) and live
//! in the config only.

use pk_cli_config::profile::{self, ProfileName};
use pk_cli_core::{confirm, output, CliError};
use serde_json::{json, Value};

use super::{Ctx, BIN, PROFILE_ENV};
use crate::cli::ProfilesCmd;
use crate::config::ProfileSettings;

pub fn run(ctx: &Ctx, cmd: &ProfilesCmd) -> Result<(), CliError> {
    match cmd {
        ProfilesCmd::List => list(ctx),
        ProfilesCmd::Add { name } => add(ctx, name),
        ProfilesCmd::Remove { name, force } => remove(ctx, name, *force),
    }
}

fn list(ctx: &Ctx) -> Result<(), CliError> {
    let list = ctx.config()?.profiles.list();
    if ctx.json() {
        output::json(&serde_json::to_value(&list).expect("profile-list/v1 serializes"));
    } else if list.profiles.is_empty() {
        println!("No profile is configured (`{BIN} profiles add <NAME>`).");
    } else {
        for p in &list.profiles {
            println!("{}", p.name);
        }
    }
    Ok(())
}

fn add(ctx: &Ctx, name: &str) -> Result<(), CliError> {
    let name = ProfileName::parse(name)?;
    let mut config = ctx.config()?;
    let added = config.profiles.add(name.clone());
    if added {
        ctx.save_config(&config)?;
    }
    let done = json!({ "profile": name.as_str(), "added": added });
    output::emit(ctx.json(), "profile-add", done, |_| {
        if added {
            println!("Added profile {name}. Store its key with `{BIN} auth login --browser --profile {name}`.");
        } else {
            println!("Profile {name} is already configured.");
        }
    });
    Ok(())
}

fn remove(ctx: &Ctx, name: &str, force: bool) -> Result<(), CliError> {
    let done = removed(ctx, name, force)?;
    output::emit(ctx.json(), "profile-remove", done.clone(), |_| {
        println!(
            "Removed profile {} and its {} stored key(s). They stay valid at ICANotes until deleted on the portal's FHIR API Access page.",
            done["profile"].as_str().unwrap_or_default(),
            done["keys_removed"]
        )
    });
    Ok(())
}

/// Its keys, its kept browser profile, then its config, after asking. The
/// config goes last, so a step that fails leaves the profile listed and
/// running the command again finishes it.
fn removed(ctx: &Ctx, name: &str, force: bool) -> Result<Value, CliError> {
    let config = ctx.config()?;
    // An unknown name is the same usage error `--profile` gives.
    let name = profile::resolve_from(Some(name), None, PROFILE_ENV, &config.profiles, BIN)?;
    confirm::gate(
        &ctx.common,
        force,
        &format!("Remove profile {name}, its stored keys and its browser profile?"),
    )?;
    let again = format!("run `{BIN} profiles remove {name} --force` again to finish");
    let prefix = format!("{}/", name.as_str());
    let mut keys = 0;
    for entry in ctx
        .vault
        .names()
        .map_err(|e| e.with_remedy(again.clone()))?
    {
        if entry.starts_with(&prefix) && ctx.vault.delete(&entry)? {
            keys += 1;
        }
    }
    let browser = (ctx.forget_browser)(&name).map_err(|e| e.with_remedy(again))?;
    let mut config = ctx.config()?;
    let settings: ProfileSettings = config.profiles.settings(&name).unwrap_or_default();
    config.profiles.remove(&name);
    ctx.save_config(&config)?;
    Ok(json!({
        "profile": name.as_str(),
        "patients_removed": settings.grants.len(),
        "keys_removed": keys,
        "browser_profile_removed": browser,
    }))
}
