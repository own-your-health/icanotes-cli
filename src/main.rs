use clap::{CommandFactory, Parser};
use pk_cli_core::info::{AuthInfo, CliInfo};
use pk_cli_core::{output, CliError};
use pk_cli_selfupdate::Updater;

use icanotes::cli::{Cli, Command};
use icanotes::commands::{self, Ctx, BIN, REPO};

/// The profiles `info` declares: the records stream, and the C-CDA
/// documents through `documents list|download`.
const PROFILES: &[&str] = &["health-records/v1", "documents/v1"];

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli) {
        std::process::exit(output::fail(&e, cli.common.json));
    }
}

fn run(cli: &Cli) -> Result<(), CliError> {
    if cli.profile.is_some() && !cli.command.uses_profile() {
        return Err(CliError::Usage(
            "this command does not run as a profile, so it takes no --profile".into(),
        )
        .with_remedy(match &cli.command {
            Command::Profiles(_) => {
                "name the profile as the argument: `icanotes profiles remove <NAME>`"
            }
            _ => "leave out --profile",
        }));
    }
    // Commands that need neither the config nor the vault run first, so
    // `info` and `completions` work wherever the binary does.
    match &cli.command {
        Command::Completions { shell } => {
            clap_complete::generate(*shell, &mut Cli::command(), BIN, &mut std::io::stdout());
            return Ok(());
        }
        Command::Info => {
            info(cli.config.clone());
            return Ok(());
        }
        Command::SelfUpdate(args) => {
            return Updater {
                repo: REPO.into(),
                binary: BIN.into(),
                target: env!("BUILD_TARGET").into(),
                current: env!("CARGO_PKG_VERSION").into(),
            }
            .run(args, cli.common.json, cli.common.quiet);
        }
        _ => {}
    }
    let ctx = Ctx::for_binary(cli.common.clone(), cli.config.clone(), cli.profile.clone())?;
    match &cli.command {
        Command::Orgs(cmd) => commands::orgs::run(&ctx, cmd),
        Command::Profiles(cmd) => commands::profiles::run(&ctx, cmd),
        Command::Auth(cmd) => commands::auth::run(&ctx, cmd),
        Command::Patients(cmd) => commands::patients::run(&ctx, cmd),
        Command::Records(cmd) => commands::records::run(&ctx, cmd),
        Command::Documents(cmd) => commands::documents::run(&ctx, cmd),
        Command::Api(args) => commands::api::run(&ctx, args),
        Command::Config(cmd) => commands::config::run(&ctx, cmd),
        Command::Completions { .. } | Command::Info | Command::SelfUpdate(_) => unreachable!(),
    }
}

fn info(config: Option<std::path::PathBuf>) {
    let mut info = CliInfo::new(
        BIN,
        env!("CARGO_PKG_VERSION"),
        &format!("https://github.com/{REPO}"),
        AuthInfo {
            required: true,
            method: "password".into(),
            login_hint: Some(format!(
                "{BIN} profiles add <NAME>, then {BIN} auth login --browser --profile <NAME> (or {BIN} auth set-credential --profile <NAME>)"
            )),
        },
        &[
            "orgs", "profiles", "auth", "patients", "records", "documents", "api", "config",
        ],
    )
    .with_profiles(PROFILES);
    // Login commands take --profile, so a records store passes one.
    info.capabilities
        .push(pk_med_records::LOGIN_PROFILES.to_string());
    let mut v = serde_json::to_value(&info).expect("CliInfo serializes to JSON");
    // Whether a key is set up, and where setup is explained; from the
    // config file only, so `info` still runs anywhere and never prompts.
    v["setup"] = commands::setup_state(config);
    output::json(&v);
}
