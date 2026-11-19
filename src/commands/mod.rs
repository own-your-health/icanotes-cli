//! Command implementations. `main` builds a [`Ctx`] and dispatches; the
//! tests build one over a vault with an in-memory key, a scratch config and
//! a stand-in for the browser window.

pub mod api;
pub mod auth;
pub mod config;
pub mod documents;
pub mod orgs;
pub mod patients;
pub mod profiles;
pub mod records;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use pk_cli_auth::vault::Vault;
use pk_cli_config::profile::{self, ProfileName};
use pk_cli_config::ConfigStore;
use pk_cli_core::{CliError, CommonArgs};
use pk_cli_secrets::CredentialStore;

use crate::cli::PatientArgs;
use crate::client::{Api, Fetch, Reqwest};
use crate::config::{Config, Grant, ProfileSettings, ORG_ID};
use crate::credential::{self, Credential};
use crate::login::Window;
use crate::ratelimit::RateLimiter;

pub const BIN: &str = "icanotes";
pub const REPO: &str = "piekstra/icanotes-cli";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Names the keychain service a run uses instead of `piekstra.icanotes`.
/// For the test suite and for dogfooding: a service that holds nothing reads
/// as signed out without a keychain prompt. With it, the vault lives beside
/// the config file (`vault.<service>/`), so a test's entries never meet the
/// real vault's key.
pub const KEYCHAIN_SERVICE_ENV: &str = "ICANOTES_KEYCHAIN_SERVICE";

/// The environment's fallback for `--profile`.
pub const PROFILE_ENV: &str = "ICANOTES_PROFILE";

/// Removes a profile's kept browser profile; whether there was one.
pub type ForgetBrowser = Box<dyn Fn(&ProfileName) -> Result<bool, CliError>>;

/// Makes the transport for one run.
pub type MakeFetch = Box<dyn Fn() -> Result<Box<dyn Fetch>, CliError>>;

pub struct Ctx {
    pub common: CommonArgs,
    pub store: ConfigStore,
    /// `--profile`, before it is checked against the config.
    pub profile: Option<String>,
    pub vault: Arc<Vault>,
    pub fetch: MakeFetch,
    pub window: Window,
    pub forget_browser: ForgetBrowser,
    /// Where the request window is kept between runs (none: in memory).
    pub rate_file: Option<PathBuf>,
}

impl Ctx {
    /// The binary's context: icanotes' vault (or one under
    /// `ICANOTES_KEYCHAIN_SERVICE`), `~/.config/icanotes`, reqwest, and
    /// Chrome in the profile's own browser profile.
    pub fn for_binary(
        common: CommonArgs,
        config: Option<PathBuf>,
        profile: Option<String>,
    ) -> Result<Self, CliError> {
        let store = ConfigStore::new(BIN).with_override(config);
        let dir = store
            .path()?
            .parent()
            .map(|d| d.to_path_buf())
            .ok_or_else(|| CliError::Other("the config path has no directory".into()))?;
        let vault = match keychain_override() {
            Some(service) => Vault::new(
                CredentialStore::new(service.clone()),
                dir.join(format!("vault.{service}")),
            ),
            None => Vault::for_binary(BIN)?,
        };
        Ok(Ctx {
            common,
            store,
            profile,
            vault: Arc::new(vault),
            fetch: Box::new(|| Ok(Box::new(Reqwest::new(BIN, VERSION)?) as Box<dyn Fetch>)),
            window: crate::login::chrome_window(),
            forget_browser: Box::new(|profile| {
                pk_cli_browser::forget_named_profile(BIN, profile.as_str()).map_err(CliError::from)
            }),
            rate_file: Some(dir.join("rate.json")),
        })
    }

    pub fn json(&self) -> bool {
        self.common.json
    }

    /// The config, its hosts checked ([`Config::check`]).
    pub fn config(&self) -> Result<Config, CliError> {
        let c: Config = self.store.load()?;
        c.check()?;
        Ok(c)
    }

    /// The config as stored, for `config show|set|unset` to show and fix.
    pub fn config_unchecked(&self) -> Result<Config, CliError> {
        self.store.load()
    }

    /// Write the config. Its grants carry patient ids, and
    /// `ConfigStore::save` (cli-common v0.33.1) writes it owner-only and
    /// whole.
    pub fn save_config(&self, config: &Config) -> Result<(), CliError> {
        self.store.save(config)
    }

    /// The profile this command runs as: `--profile`, else
    /// `ICANOTES_PROFILE`. It must be configured; there is no default.
    pub fn profile(&self, config: &Config) -> Result<ProfileName, CliError> {
        profile::resolve(self.profile.as_deref(), PROFILE_ENV, &config.profiles, BIN)
    }

    pub fn settings(
        &self,
        config: &Config,
        profile: &ProfileName,
    ) -> Result<ProfileSettings, CliError> {
        config.profiles.settings(profile)
    }

    pub fn save_settings(
        &self,
        profile: &ProfileName,
        settings: &ProfileSettings,
    ) -> Result<(), CliError> {
        let mut config = self.config()?;
        config.profiles.set_settings(profile, settings)?;
        self.save_config(&config)
    }

    /// The API client for this run, waiting at most `max_wait` for a window.
    pub fn api(&self, config: &Config, max_wait: Duration) -> Result<Api, CliError> {
        let quiet = self.common.quiet;
        let rate = RateLimiter::new(
            self.rate_file.clone(),
            max_wait,
            Box::new(move |m| {
                if !quiet {
                    eprintln!("{m}");
                }
            }),
        );
        Ok(Api::new(config.api_base(), (self.fetch)()?, rate))
    }

    /// The profile's patient `who` names, with its stored key.
    pub fn grant(
        &self,
        config: &Config,
        profile: &ProfileName,
        who: &PatientArgs,
    ) -> Result<(Grant, Credential), CliError> {
        if let Some(org) = who.org.as_deref() {
            if org != ORG_ID {
                return Err(CliError::Usage(format!(
                    "`{org}` is not an organization icanotes reaches (only `{ORG_ID}`)"
                )));
            }
        }
        let settings = self.settings(config, profile)?;
        let grant = settings
            .pick(who.patient.as_deref(), profile.as_str())?
            .clone();
        let entry = grant.entry(profile.as_str());
        let cred = credential::read(&self.vault, &entry)?.ok_or_else(|| {
            CliError::Auth(format!(
                "profile {profile} lists the patient but holds no key for them"
            ))
            .because(pk_cli_core::Reason::NoCredential)
            .with_remedy(format!(
                "run `{BIN} auth login --browser --profile {profile}` again"
            ))
        })?;
        Ok((grant, cred))
    }

    pub fn note(&self, msg: impl AsRef<str>) {
        if !self.common.quiet {
            eprintln!("{}", msg.as_ref());
        }
    }
}

fn keychain_override() -> Option<String> {
    std::env::var(KEYCHAIN_SERVICE_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// An error as one line for stderr, its remedy after it, for a command
/// whose stdout already carries its output.
pub fn report(e: &CliError) {
    eprintln!("error: {e}");
}

/// Minutes as a duration.
pub fn minutes(m: u64) -> Duration {
    Duration::from_secs(m.saturating_mul(60))
}
