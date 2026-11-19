//! A stand-in for ICANotes' patient API on 127.0.0.1, serving the synthetic
//! fixtures in `tests/fixtures`. Shared by the e2e tests and
//! `examples/mock_icanotes.rs` (for dogfooding the installed binary).
//!
//! It answers as ICANotes documents: `GET /API/<Endpoint>?userKey=&userSecret=`
//! `&patientID=&req_type=Complete|DateRange&FromDate=&ToDate=`, a JSON array
//! of entries, PatientCCD a ZIP of C-CDA XML with `documentCount`, 401 for a
//! key it does not know, 403 for a patient id that is not the key's, 400 for a
//! bad `req_type` or date, 429 past the window, and `X-RateLimit-*` headers.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub const KEY: &str = "synthetic-key-0001";
pub const SECRET: &str = "synthetic+secret/0001=";
pub const PATIENT_ID: &str = "100001";
/// A second key the mock knows, tied to another synthetic patient.
pub const OTHER_KEY: &str = "synthetic-key-0002";
pub const OTHER_SECRET: &str = "synthetic-secret-0002";
pub const OTHER_PATIENT_ID: &str = "100002";

pub const CCD: &[u8] = include_bytes!("../fixtures/ccd.xml");

/// The resource endpoints and their fixtures.
pub const ENDPOINTS: &[(&str, &[u8])] = &[
    (
        "PatientDemographics",
        include_bytes!("../fixtures/PatientDemographics.json"),
    ),
    (
        "AllergyIntolerance",
        include_bytes!("../fixtures/AllergyIntolerance.json"),
    ),
    (
        "MedicationRequest",
        include_bytes!("../fixtures/MedicationRequest.json"),
    ),
    ("Condition", include_bytes!("../fixtures/Condition.json")),
    (
        "Observation",
        include_bytes!("../fixtures/Observation.json"),
    ),
    (
        "DiagnosticReport",
        include_bytes!("../fixtures/DiagnosticReport.json"),
    ),
    (
        "Immunization",
        include_bytes!("../fixtures/Immunization.json"),
    ),
    ("Procedure", include_bytes!("../fixtures/Procedure.json")),
    ("Device", include_bytes!("../fixtures/Device.json")),
    ("CarePlan", include_bytes!("../fixtures/CarePlan.json")),
    ("Goal", include_bytes!("../fixtures/Goal.json")),
    ("CareTeam", include_bytes!("../fixtures/CareTeam.json")),
];

/// One request as the mock saw it, without the key and secret.
#[derive(Debug, Clone)]
pub struct Seen {
    pub endpoint: String,
    pub params: BTreeMap<String, String>,
    /// Whether the key and secret both came, in the query.
    pub keyed: bool,
}

#[derive(Debug)]
pub struct State {
    /// An endpoint's status in place of its answer.
    pub faults: BTreeMap<String, u16>,
    /// A body in place of an endpoint's fixture.
    pub bodies: BTreeMap<String, Vec<u8>>,
    pub limit: u32,
    pub remaining: u32,
    /// `X-RateLimit-Reset` as seconds from now (what the client reads as
    /// relative). Each request after it has passed opens a new window.
    pub reset_secs: i64,
    window_ends: i64,
    pub seen: Vec<Seen>,
    /// Keys the mock has revoked (401 from then on).
    pub revoked: Vec<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            faults: BTreeMap::new(),
            bodies: BTreeMap::new(),
            limit: 1000,
            remaining: 1000,
            reset_secs: 600,
            window_ends: 0,
            seen: Vec::new(),
            revoked: Vec::new(),
        }
    }
}

pub struct Mock {
    pub port: u16,
    pub state: Arc<Mutex<State>>,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Mock {
    /// Listen on a free port on 127.0.0.1, serving until the process ends.
    pub fn start() -> Mock {
        Self::start_on(0)
    }

    pub fn start_on(port: u16) -> Mock {
        let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind 127.0.0.1");
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = shared.clone();
                std::thread::spawn(move || {
                    let _ = serve(stream, &state);
                });
            }
        });
        Mock { port, state }
    }

    /// The API base to set as `api_base`.
    pub fn api_base(&self) -> String {
        format!("http://127.0.0.1:{}/API", self.port)
    }

    pub fn fault(&self, endpoint: &str, status: u16) {
        self.state
            .lock()
            .unwrap()
            .faults
            .insert(endpoint.into(), status);
    }

    pub fn body(&self, endpoint: &str, body: &[u8]) {
        self.state
            .lock()
            .unwrap()
            .bodies
            .insert(endpoint.into(), body.to_vec());
    }

    pub fn revoke(&self, key: &str) {
        self.state.lock().unwrap().revoked.push(key.into());
    }

    /// ICANotes' window: `limit` requests, reset `reset_secs` from the
    /// first request of a window.
    pub fn window(&self, limit: u32, reset_secs: i64) {
        let mut s = self.state.lock().unwrap();
        s.limit = limit;
        s.remaining = limit;
        s.reset_secs = reset_secs;
        s.window_ends = 0;
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.state.lock().unwrap().seen.clone()
    }
}

/// The ZIP PatientCCD answers: one C-CDA and the stylesheet ICANotes
/// publishes beside them.
pub fn ccd_zip() -> Vec<u8> {
    icanotes::zip::write(&[
        ("Synthetic_Patient_CCD.xml", CCD),
        ("CDA.xsl", b"<xsl:stylesheet version=\"1.0\"/>".as_slice()),
    ])
}

struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Answer {
    fn json(status: u16, body: &[u8]) -> Answer {
        Answer {
            status,
            headers: vec![(
                "Content-Type".into(),
                "application/json; charset=utf-8".into(),
            )],
            body: body.to_vec(),
        }
    }

    fn error(status: u16, message: &str) -> Answer {
        Answer::json(
            status,
            format!("{{\"status\":{status},\"message\":\"{message}\"}}").as_bytes(),
        )
    }
}

fn mm_dd_yyyy(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[2] == b'/'
        && b[5] == b'/'
        && s.split('/').all(|p| p.bytes().all(|c| c.is_ascii_digit()))
}

fn answer(path: &str, query: &str, state: &Mutex<State>) -> Answer {
    let params: BTreeMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let Some(endpoint) = path.strip_prefix("/API/") else {
        return Answer::error(404, "Not found");
    };
    let endpoint = endpoint.to_string();
    let mut s = state.lock().unwrap();
    let key = params.get("userKey").cloned().unwrap_or_default();
    let secret = params.get("userSecret").cloned().unwrap_or_default();
    let mut shown = params.clone();
    shown.remove("userKey");
    shown.remove("userSecret");
    s.seen.push(Seen {
        endpoint: endpoint.clone(),
        params: shown,
        keyed: !key.is_empty() && !secret.is_empty(),
    });

    // The window, counted for every request as ICANotes does.
    let t = now();
    if s.window_ends <= t {
        s.window_ends = t + s.reset_secs;
        s.remaining = s.limit;
    }
    let limited = s.remaining == 0;
    if !limited {
        s.remaining -= 1;
    }
    let rate = vec![
        ("X-RateLimit-Limit".to_string(), s.limit.to_string()),
        ("X-RateLimit-Remaining".to_string(), s.remaining.to_string()),
        (
            "X-RateLimit-Reset".to_string(),
            (s.window_ends - t).max(0).to_string(),
        ),
    ];
    let with_rate = |mut a: Answer| {
        a.headers.extend(rate.clone());
        a
    };
    if limited {
        return with_rate(Answer::error(429, "Too many requests"));
    }
    if let Some(status) = s.faults.get(&endpoint) {
        return with_rate(Answer::error(*status, "Injected fault"));
    }
    if key.is_empty() || secret.is_empty() {
        return with_rate(Answer::error(403, "Forbidden"));
    }
    let patient = match (key.as_str(), secret.as_str()) {
        (KEY, SECRET) => PATIENT_ID,
        (OTHER_KEY, OTHER_SECRET) => OTHER_PATIENT_ID,
        _ => return with_rate(Answer::error(401, "Invalid token")),
    };
    if s.revoked.contains(&key) {
        return with_rate(Answer::error(401, "Invalid token"));
    }
    if endpoint == "PatientID" {
        return with_rate(Answer::json(
            200,
            format!("[{{\"patientID\":\"{patient}\"}}]").as_bytes(),
        ));
    }
    if params.get("patientID").map(String::as_str) != Some(patient) {
        return with_rate(Answer::error(403, "Forbidden"));
    }
    match params.get("req_type").map(String::as_str) {
        Some("Complete") => {}
        Some("DateRange") => {
            let ok = ["FromDate", "ToDate"]
                .iter()
                .all(|k| params.get(*k).is_some_and(|v| mm_dd_yyyy(v)));
            if !ok {
                return with_rate(Answer::error(400, "Bad request"));
            }
        }
        _ => return with_rate(Answer::error(400, "Bad request")),
    }
    if endpoint == "PatientCCD" {
        let zip = s.bodies.get("PatientCCD").cloned().unwrap_or_else(ccd_zip);
        return with_rate(Answer {
            status: 200,
            headers: vec![
                ("Content-Type".into(), "application/zip".into()),
                ("documentCount".into(), "1".into()),
                ("fileFormat".into(), "Zip".into()),
                ("patientID".into(), patient.into()),
            ],
            body: zip,
        });
    }
    if let Some(body) = s.bodies.get(&endpoint) {
        return with_rate(Answer::json(200, body));
    }
    match ENDPOINTS.iter().find(|(e, _)| *e == endpoint) {
        // The other synthetic patient has nothing on file.
        Some(_) if patient == OTHER_PATIENT_ID => with_rate(Answer::json(200, b"[]")),
        Some((_, body)) => with_rate(Answer::json(200, body)),
        None => with_rate(Answer::error(404, "Not found")),
    }
}

fn serve(stream: TcpStream, state: &Mutex<State>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut length = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let a = if method == "GET" {
        answer(path, query, state)
    } else {
        Answer::error(405, "Method not allowed")
    };
    let mut out = stream;
    let reason = match a.status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        _ => "Error",
    };
    write!(out, "HTTP/1.1 {} {reason}\r\n", a.status)?;
    for (k, v) in &a.headers {
        write!(out, "{k}: {v}\r\n")?;
    }
    write!(
        out,
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        a.body.len()
    )?;
    out.write_all(&a.body)?;
    out.flush()
}
