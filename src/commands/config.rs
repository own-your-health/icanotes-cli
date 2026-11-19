//! `icanotes config path|show|set|unset`.

use pk_cli_core::{output, CliError};
use serde_json::json;

use super::Ctx;
use crate::cli::ConfigCmd;
use crate::config::KEYS;

pub fn run(ctx: &Ctx, cmd: &ConfigCmd) -> Result<(), CliError> {
    match cmd {
        ConfigCmd::Path => {
            let path = ctx.store.path()?;
            output::emit(
                ctx.json(),
                "config-path",
                json!({ "path": path.display().to_string() }),
                |_| println!("{}", path.display()),
            );
        }
        ConfigCmd::Show => {
            let c = ctx.config_unchecked()?;
            let v = json!({
                "api_base": c.api_base(),
                "portal_url": c.portal_url(),
                "profiles": c.profiles.list().profiles.iter().map(|p| p.name.to_string()).collect::<Vec<_>>(),
                "keys": KEYS,
            });
            output::emit(ctx.json(), "config", v, output::render);
        }
        ConfigCmd::Set { key, value } => {
            let mut c = ctx.config_unchecked()?;
            c.set(key, value)?;
            ctx.save_config(&c)?;
            output::emit(
                ctx.json(),
                "config-set",
                json!({ "key": key, "set": true }),
                |_| println!("Set {key}."),
            );
        }
        ConfigCmd::Unset { key } => {
            let mut c = ctx.config_unchecked()?;
            c.unset(key)?;
            ctx.save_config(&c)?;
            output::emit(
                ctx.json(),
                "config-unset",
                json!({ "key": key, "unset": true }),
                |_| println!("Unset {key}."),
            );
        }
    }
    Ok(())
}
