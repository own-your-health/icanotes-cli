//! The command tree.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;
use pk_cli_core::CommonArgs;
use pk_cli_secrets::SecretSourceArgs;
use pk_cli_selfupdate::SelfUpdateArgs;

/// Pull your own records from a practice on ICANotes (patientonlineportal.com)
/// through the patient API key its portal generates. Unofficial; not
/// affiliated with or endorsed by ICANotes.
#[derive(Parser, Debug)]
#[command(name = "icanotes", version, about, long_about = None)]
pub struct Cli {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Config file location (default ~/.config/icanotes/config.json).
    #[arg(long, global = true, value_name = "PATH", env = "ICANOTES_CONFIG")]
    pub config: Option<PathBuf>,

    /// The login to use: one patient-portal account, added with `icanotes
    /// profiles add`. Required by `auth`, `patients`, `records`, `documents`
    /// and `api`, and refused by every other command (`profiles remove`
    /// takes the name as its argument); `ICANOTES_PROFILE` when not given.
    /// There is no default.
    #[arg(long, global = true, value_name = "NAME")]
    pub profile: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// The organization icanotes reaches: ICANotes' patient API (org-list/v1).
    #[command(subcommand)]
    Orgs(OrgsCmd),
    /// The logins icanotes keeps, one per patient-portal account
    /// (profile-list/v1).
    #[command(subcommand)]
    Profiles(ProfilesCmd),
    /// The patient API key: generated in the portal and stored, checked,
    /// removed.
    #[command(subcommand)]
    Auth(AuthCmd),
    /// The patients whose keys a profile holds (patient-list/v1).
    #[command(subcommand)]
    Patients(PatientsCmd),
    /// A patient's records as a records-stream/v1 NDJSON stream.
    #[command(subcommand)]
    Records(RecordsCmd),
    /// The patient's C-CDA documents (documents/v1).
    #[command(subcommand)]
    Documents(DocumentsCmd),
    /// One request to the patient API as the profile's patient: `icanotes api
    /// GET 'Goal?req_type=Complete'`. The key, secret and patient id are
    /// added, and redacted from what is printed.
    Api(ApiArgs),
    /// Non-secret settings: a test server's API base, the portal's address.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Update to the latest release from GitHub.
    SelfUpdate(SelfUpdateArgs),
    /// Print a shell completion script.
    Completions { shell: Shell },
    /// Machine-readable capability discovery (cli-info/v1).
    Info,
}

impl Command {
    /// Whether the command runs as a profile, so `--profile` means
    /// something to it.
    pub fn uses_profile(&self) -> bool {
        match self {
            Command::Auth(_)
            | Command::Patients(_)
            | Command::Records(_)
            | Command::Documents(_)
            | Command::Api(_) => true,
            Command::Orgs(_)
            | Command::Profiles(_)
            | Command::Config(_)
            | Command::SelfUpdate(_)
            | Command::Completions { .. }
            | Command::Info => false,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum OrgsCmd {
    /// Every organization icanotes reaches.
    #[command(visible_alias = "ls")]
    List,
    /// The organizations whose name or other names contain every word of TEXT.
    Search { text: String },
}

#[derive(Subcommand, Debug)]
pub enum ProfilesCmd {
    /// The configured profiles (profile-list/v1).
    #[command(visible_alias = "ls")]
    List,
    /// Add a profile: one patient-portal account (`adult1`). Then `icanotes
    /// auth login --browser --profile <NAME>` stores its key.
    Add {
        /// 1 to 32 lowercase letters, digits, `-` or `_`.
        name: String,
    },
    /// Remove a profile with its stored keys and its browser profile. Asks
    /// first; --force to run without asking. The keys stay valid at ICANotes
    /// until deleted on the portal's FHIR API Access page.
    Remove {
        name: String,
        /// Remove without asking.
        #[arg(long)]
        force: bool,
    },
}

/// `auth login --timeout`'s default, in minutes.
pub const LOGIN_TIMEOUT: u64 = 15;

#[derive(Subcommand, Debug)]
pub enum AuthCmd {
    /// Open the patient portal in Chrome, in the profile's own browser
    /// profile. Sign in and generate a key on My History, FHIR API Access;
    /// icanotes reads the key and secret off the page, checks them with
    /// ICANotes and stores them in its vault, printing neither.
    Login(LoginArgs),
    /// Store a key and secret generated on the portal: `<key>:<secret>`, or
    /// the two on two lines, from stdin, --from-env or a prompt that does not
    /// echo. Never on the command line. Checked with ICANotes first, which
    /// also gives the patient id.
    SetCredential(SetCredentialArgs),
    /// What the profile holds and whether it can be used (auth-status/v1).
    /// Local, unless --check.
    Status(StatusArgs),
    /// Remove the profile's stored keys (all, or the one --patient names);
    /// --forget also deletes the vault's Keychain key once none is left.
    /// They stay valid at ICANotes until deleted on the portal.
    Logout(LogoutArgs),
}

#[derive(Args, Debug)]
pub struct LoginArgs {
    /// Sign in in a browser window (the only way: the portal's sign-in is
    /// the patient's).
    #[arg(long)]
    pub browser: bool,
    /// A name for this patient in this household (`adult1`), to pick them
    /// with --patient later.
    #[arg(long, value_name = "WHO")]
    pub label: Option<String>,
    /// How long to wait for the key to appear, in minutes.
    #[arg(long, value_name = "MIN", default_value_t = LOGIN_TIMEOUT)]
    pub timeout: u64,
}

#[derive(Args, Debug)]
pub struct SetCredentialArgs {
    #[command(flatten)]
    pub source: SecretSourceArgs,
    /// A name for this patient in this household (`adult1`).
    #[arg(long, value_name = "WHO")]
    pub label: Option<String>,
    /// Store without asking ICANotes: the patient id must then be given.
    #[arg(long, requires = "patient_id")]
    pub no_verify: bool,
    /// The patient id ICANotes ties to the key, with --no-verify.
    #[arg(long, value_name = "ID")]
    pub patient_id: Option<String>,
}

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Also ask ICANotes whether each stored key still works (one request
    /// each, counted against ICANotes' request window).
    #[arg(long)]
    pub check: bool,
}

#[derive(Args, Debug)]
pub struct LogoutArgs {
    /// Only this patient: a label or a patient id.
    #[arg(long, value_name = "ID")]
    pub patient: Option<String>,
    /// Also delete the vault's key from the Keychain once no profile holds a
    /// key, so icanotes leaves nothing there.
    #[arg(long)]
    pub forget: bool,
}

#[derive(Subcommand, Debug)]
pub enum PatientsCmd {
    /// The patients whose keys the profile holds.
    #[command(visible_alias = "ls")]
    List,
}

/// Which stored patient a command reads, and how long it may wait.
#[derive(Args, Debug, Clone, Default)]
pub struct PatientArgs {
    /// The patient: a label from `--label` or the id from `patients list`.
    /// Needed when the profile holds more than one.
    #[arg(long, value_name = "ID")]
    pub patient: Option<String>,
    /// Only this organization (`icanotes`, the only one).
    #[arg(long, value_name = "ORG")]
    pub org: Option<String>,
    /// The longest to wait for ICANotes' request window, in minutes.
    #[arg(long, value_name = "MIN", default_value_t = 16)]
    pub max_wait: u64,
}

#[derive(Subcommand, Debug)]
pub enum RecordsCmd {
    /// Everything the patient API serves for the patient (demographics,
    /// allergies, medications, conditions, observations, reports,
    /// immunizations, procedures, devices, care plans, goals, care team) and
    /// the C-CDA documents, as records-stream/v1 (what `medrec import --from
    /// icanotes` reads). 13 requests: ICANotes allows 10 per window, so an
    /// export waits out one window (about ten minutes).
    Export(ExportArgs),
}

#[derive(Args, Debug)]
pub struct ExportArgs {
    #[command(flatten)]
    pub who: PatientArgs,
    /// Only what ICANotes dates on or after this day (YYYY-MM-DD); each type
    /// has its own date (entered, performed, administered). The patient's
    /// demographics come whole.
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub since: Option<String>,
    /// Write the stream to stdout.
    #[arg(long, conflicts_with = "output")]
    pub ndjson: bool,
    /// Write the stream to DIR/records.ndjson instead. Only for a test
    /// server's synthetic patient (a mock on this machine, or ICANotes'
    /// r2test host): a patient's own records go to stdout, into medrec.
    #[arg(short, long, value_name = "DIR")]
    pub output: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum DocumentsCmd {
    /// The patient's C-CDA documents, newest first (document-list/v1). One
    /// request.
    #[command(visible_alias = "ls")]
    List {
        #[command(flatten)]
        who: PatientArgs,
    },
    /// Write one document (`-o` a file or a directory), or every one
    /// (`--all -o DIR`). One request.
    Download(DownloadArgs),
}

#[derive(Args, Debug)]
pub struct DownloadArgs {
    /// The document id from `documents list`.
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    pub id: Option<String>,
    /// Every document, into the directory -o names.
    #[arg(long)]
    pub all: bool,
    /// Where to write: a file, or a directory for --all.
    #[arg(short, long, value_name = "PATH")]
    pub output: PathBuf,
    /// Replace a file already there.
    #[arg(long)]
    pub force: bool,
    #[command(flatten)]
    pub who: PatientArgs,
}

#[derive(Args, Debug)]
pub struct ApiArgs {
    /// GET only: the patient API reads.
    pub method: String,
    /// An endpoint and its query: `AllergyIntolerance?req_type=Complete`,
    /// `PatientID`. userKey, userSecret and patientID are added.
    pub path: String,
    /// Write the answer's bytes to FILE (a C-CDA ZIP) instead of stdout.
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
    #[command(flatten)]
    pub who: PatientArgs,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Print the config file path.
    Path,
    /// Show the effective configuration.
    Show,
    /// Set a key: api_base, portal_url.
    Set { key: String, value: String },
    /// Remove a key.
    Unset { key: String },
}
