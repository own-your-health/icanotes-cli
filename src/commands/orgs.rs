//! `icanotes orgs list|search`: the one organization, ICANotes' patient API,
//! which answers for every practice on ICANotes. Local: no request.

use pk_cli_core::{output, CliError};
use pk_med_records::org_list;

use super::Ctx;
use crate::cli::OrgsCmd;
use crate::config::organization;

pub fn run(ctx: &Ctx, cmd: &OrgsCmd) -> Result<(), CliError> {
    let config = ctx.config()?;
    let org = organization(&config.api_base());
    let items = match cmd {
        OrgsCmd::List => vec![org],
        OrgsCmd::Search { text } => {
            let hay = format!("{} {} {}", org.id, org.name, org.aliases.join(" ")).to_lowercase();
            let all = text
                .split_whitespace()
                .all(|w| hay.contains(&w.to_lowercase()));
            if all {
                vec![org]
            } else {
                vec![]
            }
        }
    };
    if ctx.json() {
        output::json(&serde_json::to_value(org_list(items)).expect("org-list/v1 serializes"));
    } else if items.is_empty() {
        println!("No organization matches.");
    } else {
        for o in &items {
            println!("{}  {}  {}", o.id, o.name, o.fhir_base);
        }
    }
    Ok(())
}
