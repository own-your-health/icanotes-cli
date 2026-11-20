//! Black-box tests of the binary's surface. None reaches the Keychain or the
//! network: the keychain service is an empty test one, the config and HOME
//! are a temp dir, and every case renders help or info, works on a profile
//! with nothing stored, or is refused by argument checks before any
//! credential read.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};

/// A fresh scratch dir per call. The counter, not the clock, makes it
/// unique: parallel tests read the same instant often enough on macOS.
fn tmp() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "icanotes-cli-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn icanotes(home: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("icanotes").unwrap();
    cmd.env(
        "ICANOTES_KEYCHAIN_SERVICE",
        "piekstra.icanotes.hermetic-tests",
    )
    .env("XDG_CONFIG_HOME", home.join("config"))
    .env("HOME", home)
    .env_remove("ICANOTES_CONFIG")
    .env_remove("ICANOTES_PROFILE")
    .env_remove("NO_COLOR");
    cmd
}

fn json_of(out: &[u8]) -> Value {
    serde_json::from_slice(out).unwrap_or_else(|e| {
        panic!(
            "not one JSON document ({e}): {}",
            String::from_utf8_lossy(out)
        )
    })
}

fn stdout_json(cmd: &mut Command) -> Value {
    let out = cmd.assert().success();
    json_of(&out.get_output().stdout)
}

#[test]
fn every_command_renders_help() {
    let home = tmp();
    for args in [
        vec!["--help"],
        vec!["orgs", "--help"],
        vec!["orgs", "list", "--help"],
        vec!["orgs", "search", "--help"],
        vec!["profiles", "--help"],
        vec!["profiles", "list", "--help"],
        vec!["profiles", "add", "--help"],
        vec!["profiles", "remove", "--help"],
        vec!["auth", "--help"],
        vec!["auth", "login", "--help"],
        vec!["auth", "set-credential", "--help"],
        vec!["auth", "status", "--help"],
        vec!["auth", "logout", "--help"],
        vec!["patients", "--help"],
        vec!["patients", "list", "--help"],
        vec!["records", "--help"],
        vec!["records", "export", "--help"],
        vec!["documents", "--help"],
        vec!["documents", "list", "--help"],
        vec!["documents", "download", "--help"],
        vec!["api", "--help"],
        vec!["config", "--help"],
        vec!["config", "path", "--help"],
        vec!["config", "show", "--help"],
        vec!["config", "set", "--help"],
        vec!["config", "unset", "--help"],
        vec!["self-update", "--help"],
        vec!["completions", "--help"],
        vec!["info", "--help"],
    ] {
        icanotes(&home)
            .args(&args)
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage"));
    }
}

#[test]
fn version_and_completions() {
    let home = tmp();
    icanotes(&home)
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("icanotes "));
    for shell in ["bash", "zsh", "fish"] {
        icanotes(&home)
            .args(["completions", shell])
            .assert()
            .success()
            .stdout(predicate::str::contains("icanotes"));
    }
}

#[test]
fn info_is_cli_info() {
    let home = tmp();
    let v = stdout_json(icanotes(&home).arg("info"));
    assert_eq!(v["schema"], "cli-info/v1");
    assert_eq!(v["name"], "icanotes");
    assert_eq!(v["auth"]["method"], "password");
    assert_eq!(v["profiles"], json!(["health-records/v1", "documents/v1"]));
    assert!(v["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("login-profiles")));
    assert!(v.to_string().contains("piekstra/icanotes-cli"));
    // With nothing configured, info says so and names the setup doc.
    assert!(v["setup"]["doc"]
        .as_str()
        .unwrap()
        .ends_with("/docs/registration.md"));
    assert_eq!(v["setup"]["configured"], false);
    assert_eq!(v["setup"]["logins"], json!([]));
}

/// A newcomer is told where setup is explained by every surface that finds
/// no key: `info`, `auth status` and the error a command without one ends
/// in. None of them reads the Keychain.
#[test]
fn a_profile_with_no_key_points_at_the_setup_doc() {
    let home = tmp();
    icanotes(&home)
        .args(["profiles", "add", "me"])
        .assert()
        .success();
    let v = stdout_json(icanotes(&home).arg("info"));
    assert_eq!(v["setup"]["configured"], false);
    assert_eq!(
        v["setup"]["logins"],
        json!([{ "profile": "me", "keys": 0 }])
    );

    let s = stdout_json(icanotes(&home).args(["auth", "status", "--profile", "me", "--json"]));
    assert_eq!(s["authenticated"], false);
    assert_eq!(s["setup_doc"], v["setup"]["doc"]);
    icanotes(&home)
        .args(["auth", "status", "--profile", "me"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("Setup:").and(predicate::str::contains("registration.md")),
        );

    let out = icanotes(&home)
        .args(["documents", "list", "--profile", "me", "--json"])
        .assert()
        .code(3);
    let e = json_of(&out.get_output().stdout);
    let message = e["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("auth login --browser --profile me"),
        "{message}"
    );
    assert!(message.contains("docs/registration.md"), "{message}");
}

/// A config that does not read leaves `info` working, with the reason.
#[test]
fn info_runs_with_a_config_that_does_not_read() {
    let home = tmp();
    let dir = home.join("config").join("icanotes");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), "{ not json").unwrap();
    let v = stdout_json(icanotes(&home).arg("info"));
    assert_eq!(v["setup"]["configured"], false);
    assert!(v["setup"]["error"].is_string(), "{v}");
}

#[test]
fn orgs_list_and_search_emit_org_list() {
    let home = tmp();
    let v = stdout_json(icanotes(&home).args(["orgs", "list", "--json"]));
    assert_eq!(v["schema"], "org-list/v1");
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "icanotes");
    assert_eq!(items[0]["vendor"], "icanotes");
    assert_eq!(items[0]["fhir_base"], "https://r2.icanotes.com/API");
    let v = stdout_json(icanotes(&home).args(["orgs", "search", "icanotes", "--json"]));
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let v = stdout_json(icanotes(&home).args(["orgs", "search", "somewhere else", "--json"]));
    assert_eq!(v["items"], json!([]));
    icanotes(&home)
        .args(["orgs", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ICANotes patient API"));
}

#[test]
fn a_login_command_needs_a_configured_profile_and_never_picks_one() {
    let home = tmp();
    icanotes(&home)
        .args(["auth", "status"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("icanotes profiles add <NAME>"));
    let v = stdout_json(icanotes(&home).args(["profiles", "list", "--json"]));
    assert_eq!(v, json!({"schema": "profile-list/v1", "profiles": []}));
    let v = stdout_json(icanotes(&home).args(["profiles", "add", "adult1", "--json"]));
    assert_eq!(
        v,
        json!({"schema": "profile-add/v1", "profile": "adult1", "added": true})
    );
    for args in [
        vec!["auth", "status"],
        vec!["auth", "logout"],
        vec!["auth", "login", "--browser"],
        vec![
            "auth",
            "set-credential",
            "--from-env",
            "ICANOTES_TEST_UNSET_VAR",
        ],
        vec!["patients", "list"],
        vec!["records", "export", "--ndjson"],
        vec!["documents", "list"],
        vec!["api", "GET", "Goal?req_type=Complete"],
    ] {
        icanotes(&home)
            .args(&args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("configured: adult1"));
    }
    icanotes(&home)
        .args(["auth", "status", "--profile", "adult2"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("icanotes profiles add adult2"));
    for args in [
        vec!["orgs", "list"],
        vec!["config", "show"],
        vec!["info"],
        vec!["profiles", "remove", "adult1", "--force"],
    ] {
        icanotes(&home)
            .args(&args)
            .args(["--profile", "adult1"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("takes no --profile"));
    }
    for bad in ["Adult1", "_x", "a/b"] {
        icanotes(&home)
            .args(["profiles", "add", bad])
            .assert()
            .code(2);
    }
    let v = stdout_json(icanotes(&home).args(["profiles", "list", "--json"]));
    assert_eq!(v["profiles"], json!([{"name": "adult1"}]));
}

#[test]
fn nothing_stored_reads_as_such_without_the_keychain() {
    let home = tmp();
    icanotes(&home)
        .args(["profiles", "add", "adult1"])
        .assert()
        .success();
    let v = stdout_json(icanotes(&home).args(["auth", "status", "--profile", "adult1", "--json"]));
    assert_eq!(v["schema"], "auth-status/v1");
    assert_eq!(v["required"], true);
    assert_eq!(v["method"], "password");
    assert_eq!(v["authenticated"], false);
    assert_eq!(v["credential_in_keychain"], false);
    assert_eq!(v["browser_profile"], false);
    assert_eq!(v["profile"], "adult1");
    assert_eq!(v["patients"], json!([]));
    // --check with nothing stored sends nothing.
    let v = stdout_json(icanotes(&home).args([
        "auth",
        "status",
        "--check",
        "--profile",
        "adult1",
        "--json",
    ]));
    assert_eq!(v["authenticated"], false);
    icanotes(&home)
        .env("ICANOTES_PROFILE", "adult1")
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Patients:      none"));
    let v = stdout_json(icanotes(&home).args([
        "auth",
        "logout",
        "--forget",
        "--profile",
        "adult1",
        "--json",
    ]));
    assert_eq!(v["schema"], "auth-logout/v1");
    assert_eq!(v["keys_removed"], 0);
    // No vault was ever made, so --forget touches nothing.
    assert_eq!(v["vault_cleared"], false);
    let v =
        stdout_json(icanotes(&home).args(["patients", "list", "--profile", "adult1", "--json"]));
    assert_eq!(v["schema"], "patient-list/v1");
    assert_eq!(v["items"], json!([]));
    for args in [
        vec!["records", "export", "--ndjson"],
        vec!["documents", "list"],
        vec!["api", "GET", "PatientID"],
    ] {
        icanotes(&home)
            .args(&args)
            .args(["--profile", "adult1"])
            .assert()
            .code(3)
            .stderr(predicate::str::contains(
                "icanotes auth login --browser --profile adult1",
            ));
    }
    icanotes(&home)
        .args(["profiles", "remove", "adult1", "--json"])
        .assert()
        .code(6);
    let v =
        stdout_json(icanotes(&home).args(["profiles", "remove", "adult1", "--force", "--json"]));
    assert_eq!(v["schema"], "profile-remove/v1");
    assert_eq!(v["keys_removed"], 0);
    assert_eq!(v["browser_profile_removed"], false);
    let v = stdout_json(icanotes(&home).args(["profiles", "list", "--json"]));
    assert_eq!(v["profiles"], json!([]));
    // Nothing was ever stored, so no vault was made.
    assert!(!home
        .join("config/icanotes/vault.piekstra.icanotes.hermetic-tests")
        .exists());
}

#[test]
fn bad_arguments_are_refused_before_anything_else() {
    let home = tmp();
    icanotes(&home)
        .args(["profiles", "add", "adult1"])
        .assert()
        .success();
    for (args, code) in [
        (vec!["nope"], 2),
        (vec!["auth", "login"], 2),
        (
            vec!["auth", "login", "--browser", "--label", "has space"],
            2,
        ),
        (vec!["auth", "login", "--browser", "--timeout", "0"], 2),
        (
            vec!["auth", "set-credential", "--stdin", "--from-env", "X"],
            2,
        ),
        (vec!["auth", "set-credential", "--no-verify"], 2),
        (
            vec!["auth", "set-credential", "--patient-id", "a b", "--stdin"],
            2,
        ),
        (vec!["records", "export"], 2),
        (vec!["records", "export", "--ndjson", "--json"], 2),
        (vec!["records", "export", "--ndjson", "-o", "x"], 2),
        (
            vec!["records", "export", "--ndjson", "--since", "2024-13-01"],
            2,
        ),
        (
            vec!["records", "export", "--ndjson", "--since", "2999-01-01"],
            2,
        ),
        (
            vec!["records", "export", "--ndjson", "--org", "elsewhere"],
            2,
        ),
        (vec!["documents", "download"], 2),
        (vec!["documents", "download", "x", "--all", "-o", "d"], 2),
        (vec!["documents", "download", "a b", "-o", "d"], 2),
        (vec!["api", "POST", "Goal"], 2),
        (vec!["api", "GET", "Goal?userKey=x"], 2),
        (vec!["api", "GET", "Goal?userSecret=x"], 2),
        (vec!["api", "GET", "../Goal"], 2),
        (
            vec!["api", "GET", "https://elsewhere.example.com/API/Goal"],
            2,
        ),
        (vec!["config", "set", "nope", "x"], 2),
        (
            vec!["config", "set", "api_base", "http://r2.icanotes.com/API"],
            2,
        ),
        (
            vec!["config", "set", "api_base", "https://example.com/API"],
            2,
        ),
        (
            vec![
                "config",
                "set",
                "portal_url",
                "http://patientonlineportal.com/",
            ],
            2,
        ),
        (vec!["profiles", "add", "Adult1"], 2),
        (vec!["profiles", "remove", "nobody", "--force"], 2),
    ] {
        let mut cmd = icanotes(&home);
        cmd.args(&args);
        if args.first() != Some(&"profiles") && args.first() != Some(&"config") && args != ["nope"]
        {
            cmd.args(["--profile", "adult1"]);
        }
        cmd.write_stdin("").assert().code(code);
    }
}

#[test]
fn a_malformed_pair_is_refused_without_echoing_it() {
    let home = tmp();
    icanotes(&home)
        .args(["profiles", "add", "adult1"])
        .assert()
        .success();
    for input in [
        "has space:secret-value-1",
        "only-one-part-secret-value-1",
        ":secret-value-1",
    ] {
        let out = icanotes(&home)
            .args([
                "auth",
                "set-credential",
                "--stdin",
                "--profile",
                "adult1",
                "--json",
            ])
            .write_stdin(input)
            .assert()
            .code(2);
        let all = String::from_utf8_lossy(&out.get_output().stderr).to_string()
            + &String::from_utf8_lossy(&out.get_output().stdout);
        assert!(!all.contains("secret-value-1"), "{all}");
    }
}

#[test]
fn errors_are_one_json_document() {
    let home = tmp();
    let out = icanotes(&home)
        .args(["config", "set", "nope", "x", "--json"])
        .assert()
        .code(2);
    let v = json_of(&out.get_output().stdout);
    assert_eq!(v["error"]["code"], "usage");
}

#[test]
fn config_round_trips() {
    let home = tmp();
    icanotes(&home)
        .args([
            "config",
            "set",
            "api_base",
            "https://r2test.icanotes.com/API",
        ])
        .assert()
        .success();
    let v = stdout_json(icanotes(&home).args(["config", "show", "--json"]));
    assert_eq!(v["api_base"], "https://r2test.icanotes.com/API");
    assert_eq!(v["portal_url"], "https://patientonlineportal.com/");
    let v = stdout_json(icanotes(&home).args(["config", "path", "--json"]));
    let path = v["path"].as_str().unwrap().to_string();
    assert!(path.ends_with("icanotes/config.json"), "{path}");
    icanotes(&home)
        .args(["config", "unset", "api_base"])
        .assert()
        .success();
    let v = stdout_json(icanotes(&home).args(["config", "show", "--json"]));
    assert_eq!(v["api_base"], "https://r2.icanotes.com/API");
}

#[test]
fn a_config_edited_to_send_the_key_elsewhere_is_refused() {
    let home = tmp();
    icanotes(&home)
        .args(["profiles", "add", "adult1"])
        .assert()
        .success();
    let path = home.join("config/icanotes/config.json");
    let mut c: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    c["api_base"] = "http://example.net/API".into();
    std::fs::write(&path, serde_json::to_vec(&c).unwrap()).unwrap();
    for args in [
        &["records", "export", "--profile", "adult1", "--ndjson"][..],
        &["api", "GET", "Goal", "--profile", "adult1"],
        &["auth", "status", "--profile", "adult1", "--check"],
    ] {
        let out = icanotes(&home).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        let text = String::from_utf8_lossy(&out.stderr);
        assert!(text.contains("the config's api_base"), "{args:?}: {text}");
    }
    // `config` still shows it, and unsets it.
    let v = stdout_json(icanotes(&home).args(["config", "show", "--json"]));
    assert_eq!(v["api_base"], "http://example.net/API");
    icanotes(&home)
        .args(["config", "unset", "api_base"])
        .assert()
        .success();
    icanotes(&home)
        .args(["records", "export", "--profile", "adult1", "--ndjson"])
        .assert()
        .code(3);
}

#[test]
fn no_color_is_accepted() {
    let home = tmp();
    icanotes(&home)
        .env("NO_COLOR", "1")
        .args(["orgs", "list"])
        .assert()
        .success();
}
