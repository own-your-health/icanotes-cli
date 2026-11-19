//! ICANotes' patient API: `GET <base>/<Endpoint>?userKey=…&userSecret=…
//! &patientID=…&req_type=…`. The key and secret travel in the query, so no
//! URL is ever printed: transport errors are taken `without_url`, and every
//! message passes through the key's [`Redactor`](crate::redact::Redactor).
//!
//! Answers are JSON arrays of `{"resource": {...}, "request": {...}}`
//! entries (the published samples' shape), except `PatientID`
//! (`[{"patientID": "…"}]`) and `PatientCCD` (a ZIP of C-CDA XML files).
//! Resources are kept as the server's own text ([`RawResource`]), so a
//! decimal keeps its spelling.

use std::io::Read;
use std::time::Duration;

use pk_cli_core::{CliError, Reason};
use pk_med_fhir::RawResource;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::credential::KeyPair;
use crate::ratelimit::{span, RateLimiter, PENALTY_SECS};

/// The most of one answer read: a C-CDA archive can be large, a resource
/// list is not.
pub const MAX_BODY: u64 = 256 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(120);

/// One answer: status, headers (names lower case) and body.
#[derive(Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("bytes", &self.body.len())
            .finish_non_exhaustive()
    }
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// How a request goes out: reqwest in the binary, a stand-in in tests.
pub trait Fetch: Send + Sync {
    /// GET `url`. An error never carries the URL.
    fn get(&self, url: &str) -> Result<Response, CliError>;
}

/// reqwest, keeping the transport contract: no redirects followed (a
/// redirect would carry the key's query to wherever it points), no cookie
/// store, a bounded body.
pub struct Reqwest {
    client: reqwest::blocking::Client,
    user_agent: String,
}

impl Reqwest {
    pub fn new(bin: &str, version: &str) -> Result<Self, CliError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| CliError::Other(format!("failed to build the HTTP client: {e}")))?;
        Ok(Reqwest {
            client,
            user_agent: format!("{bin}/{version}"),
        })
    }
}

impl Fetch for Reqwest {
    fn get(&self, url: &str) -> Result<Response, CliError> {
        let origin = url::Url::parse(url)
            .map(|u| u.origin().ascii_serialization())
            .unwrap_or_else(|_| "the API".into());
        let failed =
            |why: String| CliError::Upstream(format!("the request to {origin} failed: {why}"));
        let resp = self
            .client
            .get(url)
            .header("User-Agent", &self.user_agent)
            .send()
            .map_err(|e| failed(e.without_url().to_string()))?;
        let status = resp.status().as_u16();
        let mut headers: Vec<(String, String)> = Vec::new();
        for (name, value) in resp.headers() {
            headers.push((
                name.as_str().to_ascii_lowercase(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            ));
        }
        let mut body = Vec::new();
        resp.take(MAX_BODY + 1)
            .read_to_end(&mut body)
            .map_err(|e| failed(format!("reading the answer: {e}")))?;
        if body.len() as u64 > MAX_BODY {
            return Err(failed(format!(
                "the answer is over {} MiB",
                MAX_BODY / 1024 / 1024
            )));
        }
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

/// Which of an endpoint's three query forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Range {
    /// `req_type=Complete`: everything.
    Complete,
    /// `req_type=DateRange&FromDate=MM/DD/YYYY&ToDate=MM/DD/YYYY`.
    Between {
        from: pk_cli_core::dates::Civil,
        to: pk_cli_core::dates::Civil,
    },
}

impl Range {
    fn params(&self) -> Vec<(&'static str, String)> {
        match self {
            Range::Complete => vec![("req_type", "Complete".into())],
            Range::Between { from, to } => vec![
                ("req_type", "DateRange".into()),
                ("FromDate", pk_cli_core::dates::fmt_mm_slash_dd_yyyy(*from)),
                ("ToDate", pk_cli_core::dates::fmt_mm_slash_dd_yyyy(*to)),
            ],
        }
    }
}

pub struct Api {
    base: String,
    fetch: Box<dyn Fetch>,
    rate: RateLimiter,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Api")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

/// An endpoint name: letters only, so a path can never leave the base.
pub fn check_endpoint(name: &str) -> Result<(), CliError> {
    if name.is_empty() || name.len() > 64 || !name.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(CliError::Usage(format!(
            "`{name}` is not an ICANotes endpoint name (letters only: AllergyIntolerance, PatientCCD, ...)"
        )));
    }
    Ok(())
}

/// Query parameters the CLI sets itself.
pub const RESERVED: &[&str] = &["userKey", "userSecret"];

impl Api {
    pub fn new(base: String, fetch: Box<dyn Fetch>, rate: RateLimiter) -> Self {
        Api {
            base: base.trim_end_matches('/').to_string(),
            fetch,
            rate,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn url(&self, endpoint: &str, pair: &KeyPair, params: &[(&str, String)]) -> String {
        let mut q = url::form_urlencoded::Serializer::new(String::new());
        q.append_pair("userKey", &pair.user_key);
        q.append_pair("userSecret", &pair.user_secret);
        for (k, v) in params {
            q.append_pair(k, v);
        }
        format!("{}/{}?{}", self.base, endpoint, q.finish())
    }

    /// One request under the window, its answer whatever the status.
    pub fn send(
        &self,
        endpoint: &str,
        pair: &KeyPair,
        params: &[(&str, String)],
    ) -> Result<Response, CliError> {
        check_endpoint(endpoint)?;
        let redactor = pair.redactor();
        self.rate.before()?;
        let url = self.url(endpoint, pair, params);
        let resp = self.fetch.get(&url).map_err(|e| redactor.error(e))?;
        self.rate.after(resp.status, &resp.headers);
        Ok(resp)
    }

    /// One request whose answer must be a 2xx.
    fn ok(
        &self,
        endpoint: &str,
        pair: &KeyPair,
        params: &[(&str, String)],
    ) -> Result<Response, CliError> {
        let resp = self.send(endpoint, pair, params)?;
        if (200..300).contains(&resp.status) {
            Ok(resp)
        } else {
            Err(status_error(resp.status, endpoint))
        }
    }

    /// The patient id ICANotes ties to a key: `PatientID`. Also the check
    /// that a key and secret work.
    pub fn patient_id(&self, pair: &KeyPair) -> Result<String, CliError> {
        let resp = self.ok("PatientID", pair, &[])?;
        parse_patient_id(&resp.body)
    }

    /// Every resource an endpoint answers for the patient.
    pub fn resources(
        &self,
        endpoint: &str,
        pair: &KeyPair,
        patient_id: &str,
        range: &Range,
        extra: &[(&str, String)],
    ) -> Result<Vec<RawResource>, CliError> {
        let mut params = vec![("patientID", patient_id.to_string())];
        params.extend(range.params());
        params.extend(extra.iter().cloned());
        let resp = self.ok(endpoint, pair, &params)?;
        parse_resources(&resp.body, endpoint)
    }

    /// The patient's C-CDA documents, as the ZIP ICANotes sends, and the
    /// `documentCount` it says the ZIP holds.
    pub fn ccd(
        &self,
        pair: &KeyPair,
        patient_id: &str,
        range: &Range,
    ) -> Result<(Vec<u8>, Option<u32>), CliError> {
        let mut params = vec![("patientID", patient_id.to_string())];
        params.extend(range.params());
        let resp = self.ok("PatientCCD", pair, &params)?;
        let count = resp
            .header("documentcount")
            .and_then(|v| v.trim().parse().ok());
        if !resp.body.starts_with(b"PK") {
            return Err(CliError::Upstream(format!(
                "PatientCCD answered {} bytes that are not a ZIP archive{}",
                resp.body.len(),
                resp.header("content-type")
                    .map(|t| format!(" ({t})"))
                    .unwrap_or_default()
            )));
        }
        Ok((resp.body, count))
    }
}

/// What a failed status means, naming the endpoint and never the URL.
pub fn status_error(status: u16, endpoint: &str) -> CliError {
    let bin = crate::commands::BIN;
    match status {
        400 => CliError::Upstream(format!(
            "ICANotes answered 400 (bad request) to {endpoint}"
        ))
        .because(Reason::BadRequest),
        401 => CliError::Auth(format!(
            "ICANotes refused the API key (401) at {endpoint}: it was deleted or replaced in the portal"
        ))
        .because(Reason::CredentialRefused)
        .with_remedy(format!(
            "generate a new key with `{bin} auth login --browser --profile <NAME>`"
        )),
        403 => CliError::Auth(format!(
            "ICANotes answered 403 (forbidden) to {endpoint}: it refused the key, or the practice does not allow this"
        ))
        .because(Reason::Forbidden)
        .with_remedy(format!(
            "check the key with `{bin} auth status --profile <NAME> --check`; a deleted key needs `{bin} auth login --browser --profile <NAME>`"
        )),
        404 => CliError::NotFound(format!("ICANotes answered 404 (not found) to {endpoint}")),
        429 => CliError::Upstream(format!(
            "ICANotes answered 429 (too many requests) to {endpoint} and refuses requests for {}",
            span(PENALTY_SECS)
        ))
        .because(crate::ratelimit::RATE_LIMITED)
        .with_remedy(format!("try again in {}", span(PENALTY_SECS + 60))),
        s => CliError::Upstream(format!("ICANotes answered HTTP {s} to {endpoint}")),
    }
}

#[derive(Deserialize)]
struct PatientIdRow {
    #[serde(rename = "patientID")]
    patient_id: serde_json::Value,
}

pub fn parse_patient_id(body: &[u8]) -> Result<String, CliError> {
    let rows: Vec<PatientIdRow> = serde_json::from_slice(body).map_err(|e| {
        CliError::Upstream(format!(
            "PatientID did not answer the expected list: {}",
            pk_med_fhir::describe_json_error(&e)
        ))
    })?;
    let id = match rows.as_slice() {
        [one] => match &one.patient_id {
            serde_json::Value::String(s) => s.trim().to_string(),
            serde_json::Value::Number(n) => n.to_string(),
            _ => String::new(),
        },
        [] => {
            return Err(CliError::Upstream(
                "PatientID answered an empty list: ICANotes ties no patient to this key".into(),
            ))
        }
        more => {
            return Err(CliError::Upstream(format!(
                "PatientID answered {} patients for one key",
                more.len()
            )))
        }
    };
    if !pk_med_fhir::is_id(&id) {
        return Err(CliError::Upstream(
            "PatientID answered an id that is not 1 to 256 of A-Z a-z 0-9 - .".into(),
        ));
    }
    Ok(id)
}

#[derive(Deserialize)]
struct Entry<'a> {
    #[serde(borrow, default)]
    resource: Option<&'a RawValue>,
    #[serde(rename = "resourceType", default)]
    resource_type: Option<String>,
}

#[derive(Deserialize)]
struct Bundle<'a> {
    #[serde(borrow, default)]
    entry: Vec<Entry<'a>>,
}

/// An endpoint's answer as resources: a list of entries (each a
/// `{"resource": …}` or a bare resource), a FHIR Bundle, or one resource.
/// An empty body or an empty list is no resources. An object of any other
/// shape is an error naming its keys, never its values.
pub fn parse_resources(body: &[u8], endpoint: &str) -> Result<Vec<RawResource>, CliError> {
    let text = std::str::from_utf8(body)
        .map_err(|_| CliError::Upstream(format!("{endpoint} answered text that is not UTF-8")))?;
    let trimmed = text.trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let bad = |e: serde_json::Error| {
        CliError::Upstream(format!(
            "{endpoint} did not answer JSON: {}",
            pk_med_fhir::describe_json_error(&e)
        ))
    };
    let mut out = Vec::new();
    if trimmed.starts_with('[') {
        let items: Vec<&RawValue> = serde_json::from_str(trimmed).map_err(bad)?;
        for item in items {
            out.push(entry_resource(item, endpoint)?);
        }
        return Ok(out);
    }
    let one: Entry = serde_json::from_str(trimmed).map_err(bad)?;
    if one.resource_type.as_deref() == Some("Bundle") {
        let bundle: Bundle = serde_json::from_str(trimmed).map_err(bad)?;
        for e in bundle.entry {
            if let Some(r) = e.resource {
                out.push(RawResource::parse(r.get()).map_err(|e| upstream(endpoint, e))?);
            }
        }
        return Ok(out);
    }
    if one.resource_type.is_some() {
        return Ok(vec![
            RawResource::parse(trimmed).map_err(|e| upstream(endpoint, e))?
        ]);
    }
    Err(CliError::Upstream(format!(
        "{endpoint} answered an object with {}, not resources",
        keys(trimmed)
    )))
}

fn entry_resource(item: &RawValue, endpoint: &str) -> Result<RawResource, CliError> {
    let entry: Entry = serde_json::from_str(item.get()).map_err(|e| {
        CliError::Upstream(format!(
            "{endpoint} answered a list item that is not an object: {}",
            pk_med_fhir::describe_json_error(&e)
        ))
    })?;
    let text = match (entry.resource, entry.resource_type.is_some()) {
        (Some(r), _) => r.get(),
        (None, true) => item.get(),
        (None, false) => {
            return Err(CliError::Upstream(format!(
                "{endpoint} answered a list item with {}, not a resource",
                keys(item.get())
            )))
        }
    };
    RawResource::parse(text).map_err(|e| upstream(endpoint, e))
}

fn upstream(endpoint: &str, e: CliError) -> CliError {
    CliError::Upstream(format!("{endpoint}: {}", e.message()))
}

/// An object's member names, for an error: `keys message, status`. Names
/// only, never values, and at most eight of them.
fn keys(text: &str) -> String {
    let names: Vec<String> =
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(text)
            .map(|m| {
                m.keys()
                    .take(8)
                    .map(|k| k.chars().take(40).collect())
                    .collect()
            })
            .unwrap_or_default();
    if names.is_empty() {
        "no members".into()
    } else {
        format!("keys {}", names.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Serves canned answers in order and remembers each URL asked for.
    pub struct Canned {
        pub answers: Mutex<Vec<Response>>,
        pub urls: Arc<Mutex<Vec<String>>>,
    }

    impl Fetch for Canned {
        fn get(&self, url: &str) -> Result<Response, CliError> {
            self.urls.lock().unwrap().push(url.to_string());
            let mut a = self.answers.lock().unwrap();
            if a.is_empty() {
                return Err(CliError::Upstream(format!("no answer left for {url}")));
            }
            Ok(a.remove(0))
        }
    }

    fn ok(body: &str) -> Response {
        Response {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.as_bytes().to_vec(),
        }
    }

    fn api(answers: Vec<Response>) -> (Api, Arc<Mutex<Vec<String>>>) {
        let urls = Arc::new(Mutex::new(Vec::new()));
        let fetch = Canned {
            answers: Mutex::new(answers),
            urls: urls.clone(),
        };
        let rate = RateLimiter::new(None, Duration::from_secs(1), Box::new(|_| {}));
        (
            Api::new("https://r2.example.test/API/".into(), Box::new(fetch), rate),
            urls,
        )
    }

    fn pair() -> KeyPair {
        KeyPair::new("key-one", "sec/ret+1").unwrap()
    }

    #[test]
    fn the_key_goes_in_the_query_encoded() {
        let (api, urls) = api(vec![ok(r#"[{"patientID":"1001"}]"#)]);
        assert_eq!(api.patient_id(&pair()).unwrap(), "1001");
        assert_eq!(
            urls.lock().unwrap()[0],
            "https://r2.example.test/API/PatientID?userKey=key-one&userSecret=sec%2Fret%2B1"
        );
    }

    #[test]
    fn a_numeric_patient_id_reads_too() {
        assert_eq!(
            parse_patient_id(br#"[{"patientID":1001}]"#).unwrap(),
            "1001"
        );
        assert_eq!(parse_patient_id(b"[]").unwrap_err().exit_code(), 5);
        assert_eq!(parse_patient_id(br#"{"a":1}"#).unwrap_err().exit_code(), 5);
    }

    #[test]
    fn an_error_from_the_transport_loses_the_key() {
        struct Leaky;
        impl Fetch for Leaky {
            fn get(&self, url: &str) -> Result<Response, CliError> {
                Err(CliError::Upstream(format!("failed for {url}")))
            }
        }
        let rate = RateLimiter::new(None, Duration::from_secs(1), Box::new(|_| {}));
        let api = Api::new("https://r2.example.test/API".into(), Box::new(Leaky), rate);
        let e = api.patient_id(&pair()).unwrap_err();
        let m = e.to_string();
        assert!(!m.contains("key-one") && !m.contains("sec"), "{m}");
    }

    #[test]
    fn statuses_map_to_exit_codes() {
        for (status, code) in [(400, 5), (401, 3), (403, 3), (404, 4), (429, 5), (500, 5)] {
            assert_eq!(status_error(status, "Goal").exit_code(), code, "{status}");
        }
    }

    #[test]
    fn entries_bundles_and_bare_resources_all_read() {
        let entries = br#"[{"resource":{"resourceType":"Goal","id":"g1","x":1.50},"request":{"method":"POST"}},{"resourceType":"Goal","id":"g2"}]"#;
        let r = parse_resources(entries, "Goal").unwrap();
        assert_eq!(r.len(), 2);
        assert!(r[0].raw.contains("1.50"));
        assert_eq!(r[1].id, "g2");
        let bundle = br#"{"resourceType":"Bundle","entry":[{"resource":{"resourceType":"Goal","id":"g3"}}]}"#;
        assert_eq!(parse_resources(bundle, "Goal").unwrap()[0].id, "g3");
        let one = br#"{"resourceType":"Patient","id":"p1"}"#;
        assert_eq!(
            parse_resources(one, "PatientDemographics").unwrap()[0].id,
            "p1"
        );
        assert!(parse_resources(b"  ", "Goal").unwrap().is_empty());
        assert!(parse_resources(b"[]", "Goal").unwrap().is_empty());
    }

    #[test]
    fn an_unexpected_object_names_its_keys_not_its_values() {
        let e = parse_resources(br#"{"message":"Jane Doe has no data"}"#, "Goal").unwrap_err();
        let m = e.to_string();
        assert!(m.contains("keys message"), "{m}");
        assert!(!m.contains("Jane"), "{m}");
        let e = parse_resources(br#"[{"note":"Jane"}]"#, "Goal").unwrap_err();
        assert!(!e.to_string().contains("Jane"));
        let e = parse_resources(br#"[{"resource":{"resourceType":"Goal","id":"x"},"#, "Goal")
            .unwrap_err();
        assert_eq!(e.exit_code(), 5);
    }

    #[test]
    fn a_date_range_is_month_day_year() {
        let (api, urls) = api(vec![ok("[]")]);
        let range = Range::Between {
            from: (2020, 1, 2),
            to: (2020, 3, 4),
        };
        api.resources("Goal", &pair(), "1001", &range, &[]).unwrap();
        let u = &urls.lock().unwrap()[0];
        assert!(
            u.ends_with(
                "patientID=1001&req_type=DateRange&FromDate=01%2F02%2F2020&ToDate=03%2F04%2F2020"
            ),
            "{u}"
        );
    }

    #[test]
    fn a_ccd_that_is_not_a_zip_is_refused() {
        let (api, _) = api(vec![ok(r#"{"message":"x"}"#)]);
        let e = api.ccd(&pair(), "1001", &Range::Complete).unwrap_err();
        assert_eq!(e.exit_code(), 5);
    }

    #[test]
    fn endpoint_names_stay_under_the_base() {
        assert!(check_endpoint("AllergyIntolerance").is_ok());
        for bad in ["", "../x", "a/b", "Patient.x", "x?y"] {
            assert_eq!(check_endpoint(bad).unwrap_err().exit_code(), 2, "{bad}");
        }
    }
}
