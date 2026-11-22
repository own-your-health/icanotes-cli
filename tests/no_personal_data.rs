//! The repository carries no personal data; runtime output may.
//!
//! Walks every tracked file (`git ls-files`) and looks for the *shapes*
//! personal data takes — phone numbers, street addresses, email addresses,
//! policy and loan numbers, card digits — rather than a denylist of real
//! values, which would publish exactly what it exists to keep out. Dummy
//! values follow obvious patterns (555 numbers, `example.com`, all-zero ids).

use std::process::Command;

fn tracked_files() -> Vec<String> {
    let out = Command::new("git")
        .args(["ls-files"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("git ls-files");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

fn digits_run(s: &str, min: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() {
            cur.push(c);
        } else {
            if cur.len() >= min {
                out.push(cur.clone());
            }
            cur.clear();
        }
    }
    out
}

/// A digit run is a dummy when it is all one digit, zeros then one digit
/// (`000000001`), or a 555 phone number.
fn is_dummy_number(n: &str) -> bool {
    let first = n.chars().next().unwrap();
    let head = &n[..n.len() - 1];
    n.chars().all(|c| c == first)
        || head.chars().all(|c| c == '0')
        || n.starts_with("555")
        || n.starts_with("1555")
}

/// An HL7 timestamp (`YYYYMMDD`, `YYYYMMDDhhmm`, `YYYYMMDDhhmmss`), as a
/// C-CDA writes every date: a date, not a number, and dates are not what
/// this scanner looks for (ISO dates pass it too).
fn is_hl7_timestamp(n: &str) -> bool {
    if ![8, 12, 14].contains(&n.len()) {
        return false;
    }
    let num = |a: usize, b: usize| n[a..b].parse::<u32>().unwrap_or(99);
    let (year, month, day) = (num(0, 4), num(4, 6), num(6, 8));
    let time_ok = n.len() == 8 || (num(8, 10) < 24 && num(10, 12) < 60);
    (1900..2100).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day) && time_ok
}

/// The line with each hex token of 20 or more characters that mixes digits
/// and letters (an object id or a hash in a public URL) blanked, so the
/// digit runs inside it are not read as numbers.
fn without_hex_ids(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        let hex_id = token.len() >= 20
            && token.chars().all(|c| c.is_ascii_hexdigit())
            && token.chars().any(|c| c.is_ascii_alphabetic())
            && token.chars().any(|c| c.is_ascii_digit());
        if hex_id {
            out.extend(std::iter::repeat_n('x', token.len()));
        } else {
            out.push_str(token);
        }
        token.clear();
    };
    for c in line.chars() {
        if c.is_ascii_alphanumeric() {
            token.push(c);
        } else {
            flush(&mut token, &mut out);
            out.push(c);
        }
    }
    flush(&mut token, &mut out);
    out
}

/// Hosts of public documentation whose URLs carry numeric ids (ICANotes'
/// API docs are a published Postman collection, `/view/<owner>/<id>`).
const PUBLIC_DOC_HOSTS: &[&str] = &["documenter.getpostman.com"];

/// The line with each URL on a [`PUBLIC_DOC_HOSTS`] host blanked.
fn without_public_doc_urls(line: &str) -> String {
    let mut out = line.to_string();
    for host in PUBLIC_DOC_HOSTS {
        let prefix = format!("https://{host}/");
        while let Some(start) = out.find(&prefix) {
            let end = out[start..]
                .find(|c: char| c.is_whitespace() || "\"'<>()`".contains(c))
                .map_or(out.len(), |i| start + i);
            out.replace_range(start..end, &"x".repeat(end - start));
        }
    }
    out
}

/// The line with each ISO date (`YYYY-MM-DD`) blanked, so a version and a
/// date beside it (`v0.1.0 (2026-11-19)`) do not join into a phone number.
fn without_iso_dates(line: &str) -> String {
    let b = line.as_bytes();
    let mut out = line.to_string();
    let digits =
        |i: usize, n: usize| b.len() >= i + n && b[i..i + n].iter().all(u8::is_ascii_digit);
    let mut i = 0;
    while i + 10 <= b.len() {
        let date = digits(i, 4)
            && b[i + 4] == b'-'
            && digits(i + 5, 2)
            && b[i + 7] == b'-'
            && digits(i + 8, 2)
            && (i == 0 || !b[i - 1].is_ascii_digit())
            && (i + 10 == b.len() || !b[i + 10].is_ascii_digit());
        if date
            && is_hl7_timestamp(&format!(
                "{}{}{}",
                &line[i..i + 4],
                &line[i + 5..i + 7],
                &line[i + 8..i + 10]
            ))
        {
            out.replace_range(i..i + 10, "xxxx-xx-xx");
            i += 10;
        } else {
            i += 1;
        }
    }
    out
}

/// The personal-data shapes on one line, each as `kind value`. A line
/// naming SNOMED carries public terminology codes, not personal numbers.
fn scan_line(line: &str) -> Vec<String> {
    let mut findings = Vec::new();
    let line = &without_iso_dates(&without_hex_ids(&without_public_doc_urls(line)));
    let public_codes = line.to_ascii_lowercase().contains("snomed");
    // Phone, loan, policy and card numbers: 7+ digit runs, and the
    // 3-3-4 phone shape.
    let phoneish = line
        .replace(['(', ')', '-', '.', ' '], "")
        .chars()
        .collect::<String>();
    for n in digits_run(line, 7).into_iter().chain(
        digits_run(&phoneish, 10)
            .into_iter()
            .filter(|n| n.len() <= 11),
    ) {
        if !is_dummy_number(&n) && !is_hl7_timestamp(&n) && !public_codes {
            findings.push(format!("number {n}"));
        }
    }
    // Email addresses outside the dummy domains.
    for word in line.split(|c: char| c.is_whitespace() || "\"'<>(),;".contains(c)) {
        if let Some((local, domain)) = word.split_once('@') {
            let domain = domain.split(['/', '?', '#']).next().unwrap_or(domain);
            let domain =
                domain.trim_end_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'));
            let ok = local.is_empty()
                || !domain.contains('.')
                || domain == "example.com"
                || domain == "example.org"
                || domain.ends_with(".example.com")
                || domain.ends_with(".example.org")
                || domain.ends_with(".example")
                || domain.ends_with("users.noreply.github.com");
            if !ok {
                findings.push(format!("email {word}"));
            }
        }
    }
    // Street addresses: a house number followed by a direction or a
    // street suffix, unless it is the dummy street.
    let lower = line.to_ascii_lowercase();
    for suffix in [" way", " st", " ave", " blvd", " dr", " ln", " ct", " rd"] {
        if let Some(pos) = lower.find(suffix) {
            let head = &lower[..pos];
            let has_number = head
                .split_whitespace()
                .rev()
                .take(4)
                .any(|w| w.len() >= 3 && w.chars().all(|c| c.is_ascii_digit()));
            if has_number && !lower.contains("example st") {
                findings.push(format!("address-like `{}`", line.trim()));
            }
        }
    }
    findings
}

#[test]
fn tracked_files_carry_no_personal_data() {
    let files = tracked_files();
    if files.is_empty() {
        // Not a git checkout (a packaged crate): nothing to scan.
        return;
    }
    let mut findings = Vec::new();
    for file in files {
        // This file holds the scanner's own synthetic positives.
        if file.ends_with(".lock")
            || file.starts_with("LICENSE")
            || file == "tests/no_personal_data.rs"
        {
            continue;
        }
        let path = format!("{}/{file}", env!("CARGO_MANIFEST_DIR"));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            for f in scan_line(line) {
                findings.push(format!("{file}:{}: {f}", i + 1));
            }
        }
    }
    assert!(
        findings.is_empty(),
        "personal-data shapes in tracked files:\n{}",
        findings.join("\n")
    );
}

#[test]
fn the_scanner_flags_real_shapes_and_passes_dummies() {
    for real in [
        "call (772) 404-1234",
        "loan 8123456789",
        "mail someone@gmail.com",
        "lives at 1234 NW Oak St, Somewhere",
        "member id 7342918",
        "mrn 7342918 in the snowmed field",
        "member 7342918abc",
        "phone 2015062203",
        "https://documenter.example.com/view/2843466/x",
        "call 772-404-1234 on 2026-11-19",
        "bad1234567",
    ] {
        assert!(!scan_line(real).is_empty(), "missed: {real}");
    }
    for dummy in [
        "call (555) 010-0001",
        "eSelf0000000000000000001",
        "pat@example.com",
        "100 Example St, Exampleton",
        "$111,111.11",
        "SNOMED CT 38717003 (longitudinal)",
        "https://user@fhir.example.org/x",
        "https://example.org/services/639c015049655aa96ab5b2f1/documents",
        "<effectiveTime value=\"20200102103000-0500\"/>",
        "<birthTime value=\"19000101\"/>",
        "  https://documenter.getpostman.com/view/2843466/RW1bozWC (published",
        "## v0.1.0 (2026-11-19)",
        "GITLEAKS_SHA256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    ] {
        assert!(
            scan_line(dummy).is_empty(),
            "false positive: {dummy}: {:?}",
            scan_line(dummy)
        );
    }
}
