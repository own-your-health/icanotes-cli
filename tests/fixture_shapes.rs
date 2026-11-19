//! The fixtures read as the client reads ICANotes' answers, keep the shapes
//! the export depends on, and carry only the dummy identity
//! `tests/fixtures/README.md` names. Checked positively: each
//! identity-bearing field must be a known dummy.

#[path = "support/mock.rs"]
mod mock;

use icanotes::client::{parse_patient_id, parse_resources};
use serde_json::Value;

fn values(endpoint: &str) -> Vec<Value> {
    let (_, body) = mock::ENDPOINTS
        .iter()
        .find(|(e, _)| *e == endpoint)
        .unwrap_or_else(|| panic!("no fixture for {endpoint}"));
    parse_resources(body, endpoint)
        .unwrap_or_else(|e| panic!("{endpoint}: {e}"))
        .into_iter()
        .map(|r| serde_json::from_str(&r.raw).unwrap())
        .collect()
}

#[test]
fn every_fixture_reads_as_its_type() {
    for (endpoint, _) in mock::ENDPOINTS {
        let want = if *endpoint == "PatientDemographics" {
            "Patient"
        } else {
            endpoint
        };
        for v in values(endpoint) {
            assert_eq!(v["resourceType"], *want, "{endpoint}");
        }
    }
    assert!(
        values("Device").is_empty(),
        "Device stands for none on file"
    );
}

#[test]
fn the_patient_id_is_the_dummy() {
    let body = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/PatientID.json"
    ))
    .unwrap();
    assert_eq!(parse_patient_id(&body).unwrap(), mock::PATIENT_ID);
}

#[test]
fn the_patient_is_the_dummy() {
    let p = &values("PatientDemographics")[0];
    assert_eq!(p["id"], "MU2014-SYN-100001");
    assert_eq!(p["name"][0]["family"], "Testpatient");
    assert_eq!(p["name"][0]["given"][0], "Alex");
    assert_eq!(p["birthDate"], "1900-01-01");
    assert_eq!(p["address"][0]["line"][0], "100 Example St");
    for t in p["telecom"].as_array().unwrap() {
        let v = t["value"].as_str().unwrap();
        assert!(v.contains("555") || v.ends_with("@example.com"), "{v}");
    }
    for id in p["identifier"].as_array().unwrap() {
        assert_eq!(id["value"], mock::PATIENT_ID);
    }
}

#[test]
fn every_subject_is_the_dummy_patient() {
    for (endpoint, _) in mock::ENDPOINTS {
        for v in values(endpoint) {
            for field in ["subject", "patient"] {
                if let Some(r) = v[field]["reference"].as_str() {
                    assert_eq!(r, "Patient/MU2014-SYN-100001", "{endpoint}.{field}");
                }
            }
        }
    }
}

#[test]
fn the_shapes_the_export_handles_are_kept() {
    // STU3: clinicalStatus a code, not a CodeableConcept.
    assert!(values("AllergyIntolerance")[0]["clinicalStatus"].is_string());
    assert!(values("Condition")[0]["clinicalStatus"].is_string());
    assert!(values("MedicationRequest")[0]["requester"]["agent"].is_object());
    assert!(values("Immunization")[1]["notGiven"].as_bool().unwrap());
    // The CareTeam comes without an id, as ICANotes' sample.
    assert!(values("CareTeam")[0].get("id").is_none());
    // Empty strings where ICANotes sends them.
    assert_eq!(
        values("MedicationRequest")[0]["dispenseRequest"]["validityPeriod"]["start"],
        ""
    );
}

#[test]
fn the_ccd_is_the_dummy() {
    let h = icanotes::ccd::header(mock::CCD).unwrap();
    assert_eq!(h.code.as_deref(), Some("34133-9"));
    assert_eq!(h.title.as_deref(), Some("Continuity of Care Document"));
    let text = std::str::from_utf8(mock::CCD).unwrap();
    for dummy in [
        "Testpatient",
        "100 Example St",
        "(555)010-0001",
        "19000101",
        "Example Practice",
    ] {
        assert!(text.contains(dummy), "{dummy}");
    }
    // The mock's ZIP holds it beside the stylesheet, which is not a document.
    let entries = icanotes::zip::read(&mock::ccd_zip()).unwrap();
    let a = icanotes::ccd::documents(entries, mock::PATIENT_ID);
    assert_eq!(a.documents.len(), 1);
    assert_eq!(a.other, 1);
    assert!(a.unreadable.is_empty(), "{:?}", a.unreadable);
}
