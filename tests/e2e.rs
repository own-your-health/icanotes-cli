//! Login, export, documents, `api`, status and logout in-process against the
//! mock ICANotes API on 127.0.0.1 (`tests/support/mock.rs`). Keys live in a
//! vault over a temporary directory with an in-memory key; nothing touches
//! the Keychain, a browser or the network beyond 127.0.0.1. Every value is
//! the mock's synthetic patient, under the profile `adult1`.

#[path = "support/mock.rs"]
mod mock;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pk_cli_auth::vault::{MemoryKeys, Vault};
use pk_cli_browser::Confirm;
use pk_cli_config::profile::ProfileName;
use pk_cli_config::ConfigStore;
use pk_cli_core::CliError;
use pk_cli_secrets::SecretSourceArgs;
use pk_med_records::{Item, RecordsReader};
use serde_json::Value;

use icanotes::cli::{
    ApiArgs, AuthCmd, DocumentsCmd, DownloadArgs, ExportArgs, LoginArgs, LogoutArgs, PatientArgs,
    ProfilesCmd, SetCredentialArgs, StatusArgs,
};
use icanotes::client::{Api, Fetch, Reqwest};
use icanotes::commands::{self, Ctx};
use icanotes::config::Config;
use icanotes::credential::{self, KeyPair};
use icanotes::export::{self, Plan};
use icanotes::login::WindowRequest;
use icanotes::ratelimit::RateLimiter;

use mock::{Mock, KEY, OTHER_KEY, OTHER_SECRET, PATIENT_ID, SECRET};

fn tmp() -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "icanotes-e2e-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn adult1() -> ProfileName {
    ProfileName::parse("adult1").unwrap()
}

/// The browser's part: the page shows `pair`, and the window hands it over
/// once the check accepts it, as pk-cli-browser's capture does.
fn window(pair: (&'static str, &'static str)) -> icanotes::login::Window {
    Box::new(move |req: &WindowRequest| {
        assert_eq!(req.profile.as_str(), "adult1");
        assert_eq!(req.host, "patientonlineportal.com");
        let shown = KeyPair::new(pair.0, pair.1)?;
        // The page shows it on every poll; the check asks ICANotes once.
        for _ in 0..3 {
            match (req.confirm)(&shown) {
                Confirm::Ready => return Ok(shown),
                Confirm::NotYet(_) => continue,
                _ => break,
            }
        }
        Err(CliError::Auth(
            "the window closed without a key ICANotes accepts".into(),
        ))
    })
}

fn ctx_with(dir: &Path, mock: &Mock, pair: (&'static str, &'static str)) -> Ctx {
    let store = ConfigStore::new("icanotes").with_override(Some(dir.join("config.json")));
    let mut config: Config = store.load().unwrap();
    if config.profiles.list().profiles.is_empty() {
        config.set("api_base", &mock.api_base()).unwrap();
        config.profiles.add(adult1());
        store.save(&config).unwrap();
    }
    Ctx {
        common: pk_cli_core::CommonArgs {
            quiet: true,
            json: true,
            ..Default::default()
        },
        store,
        profile: Some("adult1".into()),
        vault: Arc::new(Vault::new(MemoryKeys::new(), dir.join("vault"))),
        fetch: Box::new(|| Ok(Box::new(Reqwest::new("icanotes", "0.0.0-test")?) as Box<dyn Fetch>)),
        window: window(pair),
        forget_browser: Box::new(|_| Ok(false)),
        rate_file: Some(dir.join("rate.json")),
    }
}

fn ctx(dir: &Path, mock: &Mock) -> Ctx {
    ctx_with(dir, mock, (KEY, SECRET))
}

fn login(ctx: &Ctx, label: Option<&str>) -> Result<(), CliError> {
    commands::auth::run(
        ctx,
        &AuthCmd::Login(LoginArgs {
            browser: true,
            label: label.map(str::to_string),
            timeout: 1,
        }),
    )
}

/// Run the export the way `records export --ndjson` does, into memory.
fn export_bytes(ctx: &Ctx, since: Option<&str>) -> (Vec<u8>, Result<Option<CliError>, CliError>) {
    export_lines(ctx, since, pk_med_records::MAX_LINE_BYTES)
}

/// The same, with a lower stream line limit.
fn export_lines(
    ctx: &Ctx,
    since: Option<&str>,
    max_line_bytes: usize,
) -> (Vec<u8>, Result<Option<CliError>, CliError>) {
    let config = ctx.config().unwrap();
    let profile = ctx.profile(&config).unwrap();
    let (grant, cred) = ctx
        .grant(&config, &profile, &PatientArgs::default())
        .unwrap();
    let api = ctx.api(&config, commands::minutes(1)).unwrap();
    let plan = Plan {
        credential: &cred,
        patient: grant.context(profile.as_str()),
        organization: icanotes::config::organization(&config.api_base()),
        since: icanotes::commands::records::parse_since(since).unwrap(),
        today: pk_cli_core::dates::today(),
        pulled_at: "2026-01-01T00:00:00Z".into(),
        version: "0.0.0-test",
        max_line_bytes,
    };
    let mut out = Vec::new();
    let r = export::run(&api, &plan, &mut out, &|_| {}).map(|(_, ended)| ended);
    (out, r)
}

struct Read {
    header: pk_med_records::Header,
    resources: Vec<pk_med_fhir::RawResource>,
    trailer: pk_med_records::Trailer,
}

fn read(bytes: &[u8]) -> Read {
    let mut r = RecordsReader::new(bytes);
    let header = r.header().unwrap().clone();
    let mut resources = Vec::new();
    let mut trailer = None;
    for item in r {
        match item {
            Ok(Item::Resource(res)) => resources.push(res),
            Ok(Item::Trailer(t)) => trailer = Some(t),
            // The reader reports a stream that ended early as an error
            // carrying its trailer.
            Err(pk_med_records::StreamError::Incomplete { trailer: t }) => trailer = Some(*t),
            Err(e) => panic!("the stream did not read: {e}"),
        }
    }
    Read {
        header,
        resources,
        trailer: trailer.expect("a trailer"),
    }
}

fn count(r: &Read, t: &str) -> u64 {
    r.trailer.counts.get(t).copied().unwrap_or(0)
}

#[test]
fn a_browser_login_stores_the_key_the_page_shows() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    login(&ctx, Some("self")).unwrap();
    let config = ctx.config().unwrap();
    let settings = config
        .profiles
        .settings::<icanotes::config::ProfileSettings>(&adult1())
        .unwrap();
    assert_eq!(settings.grants.len(), 1);
    let g = &settings.grants[0];
    assert_eq!(g.id, PATIENT_ID);
    assert_eq!(g.label.as_deref(), Some("self"));
    assert_eq!(g.method, "browser");
    let cred = credential::read(&ctx.vault, &g.entry("adult1"))
        .unwrap()
        .unwrap();
    assert_eq!(cred.user_key, KEY);
    assert_eq!(cred.user_secret, SECRET);
    assert_eq!(cred.patient_id, PATIENT_ID);
    // One PatientID request: the check, remembered for the window's polls.
    let seen = mock.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].endpoint, "PatientID");
    // The config holds no key or secret.
    let text = std::fs::read_to_string(dir.join("config.json")).unwrap();
    assert!(!text.contains(KEY) && !text.contains(SECRET));
    // A second login of the same patient keeps the label.
    login(&ctx, None).unwrap();
    let config = ctx.config().unwrap();
    let settings = config
        .profiles
        .settings::<icanotes::config::ProfileSettings>(&adult1())
        .unwrap();
    assert_eq!(settings.grants.len(), 1);
    assert_eq!(settings.grants[0].label.as_deref(), Some("self"));
}

#[test]
fn a_key_icanotes_refuses_is_never_stored() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx_with(
        &dir,
        &mock,
        ("synthetic-wrong-key", "synthetic-wrong-secret"),
    );
    let e = login(&ctx, None).unwrap_err();
    assert_eq!(e.exit_code(), 3);
    assert!(!e.to_string().contains("synthetic-wrong"));
    let config = ctx.config().unwrap();
    let settings = config
        .profiles
        .settings::<icanotes::config::ProfileSettings>(&adult1())
        .unwrap();
    assert!(settings.grants.is_empty());
    assert!(ctx.vault.names().unwrap().is_empty());
    // The page kept showing it: asked once.
    assert_eq!(mock.seen().len(), 1);
}

#[test]
fn set_credential_checks_the_key_and_takes_the_patient_id_from_icanotes() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    let var = "ICANOTES_E2E_SET_CREDENTIAL";
    std::env::set_var(var, format!("{OTHER_KEY}:{OTHER_SECRET}\n"));
    commands::auth::run(
        &ctx,
        &AuthCmd::SetCredential(SetCredentialArgs {
            source: SecretSourceArgs {
                stdin: false,
                from_env: Some(var.into()),
            },
            label: Some("adult2".into()),
            no_verify: false,
            patient_id: None,
        }),
    )
    .unwrap();
    std::env::remove_var(var);
    let config = ctx.config().unwrap();
    let settings = config
        .profiles
        .settings::<icanotes::config::ProfileSettings>(&adult1())
        .unwrap();
    assert_eq!(settings.grants[0].id, mock::OTHER_PATIENT_ID);
    assert_eq!(settings.grants[0].method, "stdin");

    // A wrong patient id with the check is refused before storing.
    let var = "ICANOTES_E2E_SET_CREDENTIAL_WRONG";
    std::env::set_var(var, format!("{KEY}:{SECRET}"));
    let e = commands::auth::run(
        &ctx,
        &AuthCmd::SetCredential(SetCredentialArgs {
            source: SecretSourceArgs {
                stdin: false,
                from_env: Some(var.into()),
            },
            label: None,
            no_verify: false,
            patient_id: Some("999".into()),
        }),
    )
    .unwrap_err();
    std::env::remove_var(var);
    assert_eq!(e.exit_code(), 2);
    let config = ctx.config().unwrap();
    let settings = config
        .profiles
        .settings::<icanotes::config::ProfileSettings>(&adult1())
        .unwrap();
    assert_eq!(settings.grants.len(), 1);
}

#[test]
fn an_export_carries_every_endpoint_and_the_ccd() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    login(&ctx, Some("self")).unwrap();
    let before = mock.seen().len();
    let (bytes, r) = export_bytes(&ctx, None);
    assert!(r.unwrap().is_none());
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(
        !text.contains(KEY) && !text.contains(SECRET),
        "the key reached the stream"
    );
    let got = read(&bytes);
    assert_eq!(got.header.fhir_version, "3.0.1");
    assert_eq!(got.header.patient.id, PATIENT_ID);
    assert_eq!(got.header.patient.label.as_deref(), Some("self"));
    assert_eq!(got.header.organization.id, "icanotes");
    assert!(got.trailer.complete, "{:?}", got.trailer);
    assert!(got.trailer.skipped.is_empty(), "{:?}", got.trailer.skipped);
    for (t, n) in [
        ("Patient", 1),
        ("AllergyIntolerance", 2),
        ("MedicationRequest", 2),
        ("Condition", 2),
        ("Observation", 2),
        ("DiagnosticReport", 1),
        ("Immunization", 2),
        ("Procedure", 1),
        ("Device", 0),
        ("CarePlan", 1),
        ("Goal", 1),
        ("CareTeam", 1),
        ("DocumentReference", 1),
        ("Binary", 1),
    ] {
        assert_eq!(count(&got, t), n, "{t}");
    }
    // The CareTeam ICANotes sends without an id goes in as sent.
    let team = got
        .resources
        .iter()
        .find(|r| r.resource_type == "CareTeam")
        .unwrap();
    assert_eq!(team.id, "");
    assert!(!team.raw.contains("\"id\":\"icanotes"), "{}", team.raw);
    // The document's Binary carries the C-CDA as ICANotes sent it.
    let bin = got
        .resources
        .iter()
        .find(|r| r.resource_type == "Binary")
        .unwrap();
    let v: Value = serde_json::from_str(&bin.raw).unwrap();
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD
        .decode(v["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(data, mock::CCD);
    assert_eq!(v["contentType"], "application/xml");
    let docref = got
        .resources
        .iter()
        .find(|r| r.resource_type == "DocumentReference")
        .unwrap();
    let d: Value = serde_json::from_str(&docref.raw).unwrap();
    assert_eq!(
        d["content"][0]["attachment"]["url"],
        format!("Binary/{}", bin.id)
    );
    assert_eq!(d["subject"]["reference"], "Patient/MU2014-SYN-100001");

    // 13 requests, each with the key in the query and the patient's id.
    let seen = &mock.seen()[before..];
    assert_eq!(seen.len(), export::REQUESTS);
    assert!(seen.iter().all(|s| s.keyed));
    assert!(seen
        .iter()
        .all(|s| s.params.get("patientID").map(String::as_str) == Some(PATIENT_ID)));
    assert!(seen.iter().all(|s| s.params["req_type"] == "Complete"));
    let condition = seen.iter().find(|s| s.endpoint == "Condition").unwrap();
    assert_eq!(condition.params["cat_type"], "All");

    // A second export writes the same ids.
    let (again, _) = export_bytes(&ctx, None);
    let ids = |r: &Read| {
        r.resources
            .iter()
            .map(|x| (x.resource_type.clone(), x.id.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&read(&again)), ids(&got));
}

#[test]
fn since_asks_for_a_date_range_and_keeps_demographics_whole() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    login(&ctx, None).unwrap();
    let before = mock.seen().len();
    let (bytes, r) = export_bytes(&ctx, Some("2020-01-02"));
    assert!(r.unwrap().is_none());
    let got = read(&bytes);
    assert_eq!(got.header.since.as_deref(), Some("2020-01-02"));
    assert_eq!(got.header.since_unbounded, vec!["Patient".to_string()]);
    let seen = &mock.seen()[before..];
    let demo = seen
        .iter()
        .find(|s| s.endpoint == "PatientDemographics")
        .unwrap();
    assert_eq!(demo.params["req_type"], "Complete");
    let today = pk_cli_core::dates::fmt_mm_slash_dd_yyyy(pk_cli_core::dates::today());
    for s in seen.iter().filter(|s| s.endpoint != "PatientDemographics") {
        assert_eq!(s.params["req_type"], "DateRange", "{}", s.endpoint);
        assert_eq!(s.params["FromDate"], "01/02/2020", "{}", s.endpoint);
        assert_eq!(s.params["ToDate"], today, "{}", s.endpoint);
    }
}

#[test]
fn a_failed_endpoint_is_a_skip_and_the_export_goes_on() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    login(&ctx, None).unwrap();
    mock.fault("Goal", 500);
    mock.fault("Device", 404);
    mock.fault("Immunization", 403);
    let (bytes, r) = export_bytes(&ctx, None);
    assert!(r.unwrap().is_none());
    let got = read(&bytes);
    assert!(got.trailer.complete);
    let skipped: Vec<(String, String)> = got
        .trailer
        .skipped
        .iter()
        .map(|s| (s.search.clone().unwrap_or_default(), s.code.clone()))
        .collect();
    assert!(
        skipped.contains(&("Goal?req_type=Complete".into(), "upstream".into())),
        "{skipped:?}"
    );
    assert!(
        skipped.contains(&("Device?req_type=Complete".into(), "not_found".into())),
        "{skipped:?}"
    );
    assert!(
        skipped.contains(&("Immunization?req_type=Complete".into(), "forbidden".into())),
        "{skipped:?}"
    );
    assert_eq!(count(&got, "Goal"), 0);
    assert_eq!(count(&got, "CareTeam"), 1);
    assert_eq!(count(&got, "Binary"), 1);
    // A skip names no key, secret or patient id.
    let line = serde_json::to_string(&got.trailer).unwrap();
    assert!(!line.contains(KEY) && !line.contains(SECRET) && !line.contains(PATIENT_ID));
}

#[test]
fn resources_with_empty_ids_are_written_as_sent() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    login(&ctx, None).unwrap();
    // ICANotes writes empty strings for absent values.
    mock.body(
        "PatientDemographics",
        br#"[{"resource":{"resourceType":"Patient","id":"","name":[{"family":"Testpatient"}]},"request":{"method":"POST"}}]"#,
    );
    mock.body(
        "CareTeam",
        br#"[{"resource":{"resourceType":"CareTeam","id":null,"status":"active"}},{"resource":{"resourceType":"CareTeam","status":"inactive"}}]"#,
    );
    let (bytes, r) = export_bytes(&ctx, None);
    assert!(r.unwrap().is_none());
    let got = read(&bytes);
    assert!(got.trailer.complete, "{:?}", got.trailer);
    assert_eq!(count(&got, "Patient"), 1);
    assert_eq!(count(&got, "CareTeam"), 2);
    let patient = got
        .resources
        .iter()
        .find(|r| r.resource_type == "Patient")
        .unwrap();
    assert_eq!(patient.raw.matches("\"id\"").count(), 1, "{}", patient.raw);
}

#[test]
fn a_document_too_large_for_a_line_is_skipped_whole() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    login(&ctx, None).unwrap();
    // The synthetic C-CDA is a few KiB; as base64 in a Binary it is over a
    // 3000-byte line, and every other resource is under it.
    let (bytes, r) = export_lines(&ctx, None, 3000);
    assert!(r.unwrap().is_none());
    let got = read(&bytes);
    assert!(got.trailer.complete);
    assert_eq!(count(&got, "DocumentReference"), 0);
    assert_eq!(count(&got, "Binary"), 0);
    assert_eq!(count(&got, "CareTeam"), 1);
    let skip = got
        .trailer
        .skipped
        .iter()
        .find(|s| s.code == "too_large")
        .expect("a too_large skip");
    assert_eq!(skip.resource_type.as_deref(), Some("DocumentReference"));
    assert!(
        skip.reason.contains("documents download"),
        "{}",
        skip.reason
    );
}

#[test]
fn a_key_refused_mid_export_ends_the_stream_incomplete() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    login(&ctx, None).unwrap();
    mock.fault("Condition", 401);
    let (bytes, r) = export_bytes(&ctx, None);
    let ended = r.unwrap().expect("the export ended early");
    assert_eq!(ended.exit_code(), 3);
    let got = read(&bytes);
    assert!(!got.trailer.complete);
    assert_eq!(got.trailer.error.as_ref().unwrap().exit_code, Some(3));
    assert_eq!(count(&got, "MedicationRequest"), 2);
    assert_eq!(count(&got, "Observation"), 0);
}

#[test]
fn a_key_refused_before_the_header_writes_nothing() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    login(&ctx, None).unwrap();
    mock.revoke(KEY);
    let (bytes, r) = export_bytes(&ctx, None);
    let e = r.unwrap_err();
    assert_eq!(e.exit_code(), 3);
    assert!(bytes.is_empty());
    assert!(e
        .remedy()
        .unwrap_or_default()
        .contains("auth login --browser"));
}

#[test]
fn a_429_ends_the_export_and_the_next_run_sends_nothing() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    login(&ctx, None).unwrap();
    mock.fault("MedicationRequest", 429);
    let (bytes, r) = export_bytes(&ctx, None);
    let ended = r.unwrap().expect("the export ended early");
    assert_eq!(ended.exit_code(), 5);
    let got = read(&bytes);
    assert!(!got.trailer.complete);
    assert_eq!(count(&got, "AllergyIntolerance"), 2);
    // The block is kept in rate.json: the next run refuses before sending.
    let before = mock.seen().len();
    let (bytes, r) = export_bytes(&ctx, None);
    assert_eq!(r.unwrap_err().exit_code(), 5);
    assert!(bytes.is_empty());
    assert_eq!(mock.seen().len(), before);
}

#[test]
fn the_export_waits_out_icanotes_window_of_ten() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    // A two-second window keeps the test short; ICANotes' is ten minutes.
    // Set before the login's request, so every answer counts in it.
    mock.window(10, 2);
    login(&ctx, None).unwrap();
    let started = std::time::Instant::now();
    let (bytes, r) = export_bytes(&ctx, None);
    let ended = r.unwrap();
    assert!(ended.is_none(), "{:?}", ended.map(|e| e.to_string()));
    let got = read(&bytes);
    assert!(got.trailer.complete, "{:?}", got.trailer);
    assert_eq!(count(&got, "Binary"), 1);
    assert!(started.elapsed() >= std::time::Duration::from_secs(1));
}

#[test]
fn a_window_longer_than_max_wait_is_refused_not_waited() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    login(&ctx, None).unwrap();
    mock.window(3, 600);
    let config = ctx.config().unwrap();
    let profile = ctx.profile(&config).unwrap();
    let (_, cred) = ctx
        .grant(&config, &profile, &PatientArgs::default())
        .unwrap();
    let rate = RateLimiter::new(
        Some(dir.join("rate.json")),
        std::time::Duration::from_secs(60),
        Box::new(|_| {}),
    );
    let api = Api::new(
        config.api_base(),
        Box::new(Reqwest::new("icanotes", "0").unwrap()),
        rate,
    );
    for _ in 0..3 {
        api.patient_id(&cred.pair()).unwrap();
    }
    let e = api.patient_id(&cred.pair()).unwrap_err();
    assert_eq!(e.exit_code(), 5);
    assert!(e.remedy().unwrap_or_default().contains("--max-wait"));
}

#[test]
fn documents_download_writes_the_ccd() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    login(&ctx, None).unwrap();
    let out = dir.join("docs");
    commands::documents::run(
        &ctx,
        &DocumentsCmd::Download(DownloadArgs {
            id: None,
            all: true,
            output: out.clone(),
            force: false,
            who: PatientArgs::default(),
        }),
    )
    .unwrap();
    let files: Vec<_> = std::fs::read_dir(&out)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let name = files[0].to_string_lossy().into_owned();
    // The day, the title and the end of the document's id.
    assert!(
        name.starts_with("2020-01-02_Continuity_of_Care_Document_") && name.ends_with(".xml"),
        "{name}"
    );
    assert_eq!(
        name.len(),
        "2020-01-02_Continuity_of_Care_Document_".len() + 8 + 4
    );
    assert_eq!(std::fs::read(out.join(&name)).unwrap(), mock::CCD);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(out.join(&name))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "the file is owner-only");
    }
    // Again without --force: the file is there.
    let e = commands::documents::run(
        &ctx,
        &DocumentsCmd::Download(DownloadArgs {
            id: None,
            all: true,
            output: out.clone(),
            force: false,
            who: PatientArgs::default(),
        }),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 2);
    // An id no document has.
    let e = commands::documents::run(
        &ctx,
        &DocumentsCmd::Download(DownloadArgs {
            id: Some("ccd-000000000000000000000000".into()),
            all: false,
            output: dir.join("one.xml"),
            force: false,
            who: PatientArgs::default(),
        }),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 4);
}

#[test]
fn api_passthrough_redacts_the_key_from_what_it_writes() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    login(&ctx, None).unwrap();
    // A server that echoes the query back.
    mock.body(
        "Goal",
        format!(
            "[{{\"echo\":\"userKey={KEY}&userSecret={}\"}}]",
            url::form_urlencoded::byte_serialize(SECRET.as_bytes()).collect::<String>()
        )
        .as_bytes(),
    );
    let file = dir.join("goal.json");
    commands::api::run(
        &ctx,
        &ApiArgs {
            method: "GET".into(),
            path: "Goal?req_type=Complete".into(),
            output: Some(file.clone()),
            who: PatientArgs::default(),
        },
    )
    .unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(!text.contains(KEY), "{text}");
    assert!(
        !text.contains("synthetic%2Bsecret") && !text.contains(SECRET),
        "{text}"
    );
    assert!(text.contains("[redacted]"), "{text}");
    // Echoed JSON-escaped and lowercase-encoded: still nothing, in the
    // bytes or in what they parse to.
    let escaped = SECRET
        .replace('/', "\\/")
        .replace('+', "\\u002B")
        .replace('=', "\\u003d");
    let lower: String = url::form_urlencoded::byte_serialize(SECRET.as_bytes())
        .collect::<String>()
        .to_ascii_lowercase();
    mock.body(
        "Goal",
        format!("[{{\"a\":\"{escaped}\",\"b\":\"x?userSecret={lower}&y\"}}]").as_bytes(),
    );
    commands::api::run(
        &ctx,
        &ApiArgs {
            method: "GET".into(),
            path: "Goal?req_type=Complete".into(),
            output: Some(file.clone()),
            who: PatientArgs::default(),
        },
    )
    .unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    for shown in [text.clone(), parsed.to_string()] {
        assert!(!shown.contains("synthetic"), "{shown}");
        assert!(!shown.contains("secret"), "{shown}");
    }
    let seen = mock.seen();
    let last = seen.last().unwrap();
    assert_eq!(last.endpoint, "Goal");
    assert_eq!(last.params["patientID"], PATIENT_ID);
    // The key is never taken from the path.
    let e = commands::api::run(
        &ctx,
        &ApiArgs {
            method: "GET".into(),
            path: "Goal?userKey=x".into(),
            output: None,
            who: PatientArgs::default(),
        },
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 2);
}

#[test]
fn a_transport_error_names_no_url_or_key() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    login(&ctx, None).unwrap();
    // Nothing listens on port 9 of 127.0.0.1.
    let mut config = ctx.config().unwrap();
    config.set("api_base", "http://127.0.0.1:9/API").unwrap();
    ctx.save_config(&config).unwrap();
    let (bytes, r) = export_bytes(&ctx, None);
    let e = r.unwrap_err();
    assert!(bytes.is_empty());
    let text = format!("{e} {:?}", e);
    assert!(
        !text.contains(KEY) && !text.contains(SECRET) && !text.contains("userKey"),
        "{text}"
    );
}

#[test]
fn status_check_logout_and_profile_removal() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    let (status, v) = commands::auth::status_of(&ctx, false).unwrap();
    assert!(!status.authenticated);
    assert_eq!(v["patients"].as_array().unwrap().len(), 0);
    login(&ctx, Some("self")).unwrap();
    let (status, v) = commands::auth::status_of(&ctx, false).unwrap();
    assert!(status.authenticated);
    assert_eq!(v["patients"][0]["credential_in_keychain"], true);
    assert!(v["patients"][0].get("works").is_none());
    let (status, v) = commands::auth::status_of(&ctx, true).unwrap();
    assert!(status.authenticated);
    assert_eq!(v["patients"][0]["works"], true);
    mock.revoke(KEY);
    let (status, v) = commands::auth::status_of(&ctx, true).unwrap();
    assert!(!status.authenticated);
    assert_eq!(v["patients"][0]["works"], false);
    let text = v.to_string();
    assert!(!text.contains(KEY) && !text.contains(SECRET));

    commands::auth::run(&ctx, &AuthCmd::Status(StatusArgs { check: false })).unwrap();
    let keys_before = ctx.vault.names().unwrap().len();
    assert_eq!(keys_before, 1);
    commands::auth::run(
        &ctx,
        &AuthCmd::Logout(LogoutArgs {
            patient: None,
            forget: true,
        }),
    )
    .unwrap();
    assert!(ctx.vault.names().unwrap().is_empty());
    let (status, _) = commands::auth::status_of(&ctx, false).unwrap();
    assert!(!status.authenticated);

    // Removing the profile takes its keys and its config.
    login(&ctx, None).unwrap_err(); // the key is revoked now
    let ctx2 = ctx_with(&dir, &mock, (OTHER_KEY, OTHER_SECRET));
    login(&ctx2, None).unwrap();
    assert_eq!(ctx2.vault.names().unwrap().len(), 1);
    commands::profiles::run(
        &ctx2,
        &ProfilesCmd::Remove {
            name: "adult1".into(),
            force: true,
        },
    )
    .unwrap();
    assert!(ctx2.vault.names().unwrap().is_empty());
    assert!(ctx2.config().unwrap().profiles.list().profiles.is_empty());
}

#[test]
fn export_needs_a_destination_and_a_known_patient() {
    let mock = Mock::start();
    let dir = tmp();
    let ctx = ctx(&dir, &mock);
    let e = commands::records::run(
        &ctx,
        &icanotes::cli::RecordsCmd::Export(ExportArgs {
            who: PatientArgs::default(),
            since: None,
            ndjson: false,
            output: None,
        }),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 2);
    // No key stored yet: exit 3 with the way to store one.
    let e = commands::records::run(
        &ctx,
        &icanotes::cli::RecordsCmd::Export(ExportArgs {
            who: PatientArgs::default(),
            since: None,
            ndjson: false,
            output: Some(dir.join("out")),
        }),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 3, "{e}");
    assert!(mock.seen().is_empty());
}
