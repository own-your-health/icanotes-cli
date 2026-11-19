//! `records export`: every endpoint ICANotes' patient API serves for the
//! patient, as one `records-stream/v1` stream, the C-CDA documents last as
//! DocumentReference + Binary pairs.
//!
//! The order is fixed: `PatientDemographics` first, before the header, so a
//! key ICANotes refuses is an ordinary error (exit 3) with nothing on
//! stdout. After the header, a failed endpoint is a `Skip` and the export
//! goes on, except for what no later request can change: the key refused
//! (401), a 429, or a window this run may not wait for, which end the
//! stream incomplete.

use std::io::Write;

use pk_cli_core::dates::Civil;
use pk_cli_core::CliError;
use pk_med_records::stream::skip;
use pk_med_records::{Finished, Header, PatientContext, RecordsWriter, Skip, Source};

use crate::ccd;
use crate::client::{Api, Range};
use crate::credential::Credential;

/// The FHIR release ICANotes' patient API shapes its resources on: STU3
/// (`clinicalStatus` as a code, `MedicationRequest.requester.agent`,
/// `Immunization.notGiven`), as its published samples show.
pub const FHIR_VERSION: &str = "3.0.1";

/// One endpoint the export reads.
#[derive(Debug, Clone, Copy)]
pub struct Search {
    pub endpoint: &'static str,
    /// Parameters of its own (`cat_type=All`).
    pub extra: &'static [(&'static str, &'static str)],
    /// The FHIR type it answers.
    pub resource_type: &'static str,
}

/// The patient's demographics: only `req_type=Complete`.
pub const DEMOGRAPHICS: Search = Search {
    endpoint: "PatientDemographics",
    extra: &[],
    resource_type: "Patient",
};

/// The resource endpoints, after the demographics, in order. Each takes a
/// date range ICANotes defines per type (entered into the record, for most).
pub const SEARCHES: &[Search] = &[
    Search {
        endpoint: "AllergyIntolerance",
        extra: &[],
        resource_type: "AllergyIntolerance",
    },
    Search {
        endpoint: "MedicationRequest",
        extra: &[],
        resource_type: "MedicationRequest",
    },
    Search {
        endpoint: "Condition",
        extra: &[("cat_type", "All")],
        resource_type: "Condition",
    },
    Search {
        endpoint: "Observation",
        extra: &[("cat_type", "All")],
        resource_type: "Observation",
    },
    Search {
        endpoint: "DiagnosticReport",
        extra: &[],
        resource_type: "DiagnosticReport",
    },
    Search {
        endpoint: "Immunization",
        extra: &[],
        resource_type: "Immunization",
    },
    Search {
        endpoint: "Procedure",
        extra: &[],
        resource_type: "Procedure",
    },
    Search {
        endpoint: "Device",
        extra: &[],
        resource_type: "Device",
    },
    Search {
        endpoint: "CarePlan",
        extra: &[],
        resource_type: "CarePlan",
    },
    Search {
        endpoint: "Goal",
        extra: &[],
        resource_type: "Goal",
    },
    Search {
        endpoint: "CareTeam",
        extra: &[],
        resource_type: "CareTeam",
    },
];

pub const CCD_ENDPOINT: &str = "PatientCCD";

/// Requests one export makes: the demographics, the searches, the C-CDA.
pub const REQUESTS: usize = SEARCHES.len() + 2;

impl Search {
    fn extra(&self) -> Vec<(&'static str, String)> {
        self.extra
            .iter()
            .map(|(k, v)| (*k, v.to_string()))
            .collect()
    }

    /// The search as a skip names it: no patient id, no key.
    pub fn label(&self, range: &Range) -> String {
        let mut q = vec![match range {
            Range::Complete => "req_type=Complete".to_string(),
            Range::Between { .. } => "req_type=DateRange".to_string(),
        }];
        q.extend(self.extra.iter().map(|(k, v)| format!("{k}={v}")));
        format!("{}?{}", self.endpoint, q.join("&"))
    }
}

/// What the export needs besides the API.
pub struct Plan<'a> {
    pub credential: &'a Credential,
    pub patient: PatientContext,
    pub organization: pk_med_records::Organization,
    pub since: Option<Civil>,
    pub today: Civil,
    pub pulled_at: String,
    pub version: &'a str,
    /// The longest stream line: [`pk_med_records::MAX_LINE_BYTES`], or
    /// less in a test.
    pub max_line_bytes: usize,
}

impl Plan<'_> {
    fn range(&self) -> Range {
        match self.since {
            Some(from) => Range::Between {
                from,
                to: self.today,
            },
            None => Range::Complete,
        }
    }
}

/// Whether an error ends the export rather than skipping one endpoint.
fn ends_export(e: &CliError) -> bool {
    matches!(
        e.reason(),
        Some(pk_cli_core::Reason::CredentialRefused) | Some(crate::ratelimit::RATE_LIMITED)
    )
}

fn skip_for(label: &str, e: &CliError) -> Skip {
    let code = match e.exit_code() {
        3 => skip::FORBIDDEN,
        4 => skip::NOT_FOUND,
        _ => skip::UPSTREAM,
    };
    Skip::search(label, code, e.message().to_string())
}

/// Run the export into `out`. An error before the header leaves `out`
/// empty and is returned; after it, the stream ends with its trailer, and
/// the error that ended it early, if one did, comes back beside what was
/// written (`Err` only when writing itself failed).
pub fn run<W: Write>(
    api: &Api,
    plan: &Plan,
    out: W,
    note: &dyn Fn(&str),
) -> Result<(Finished, Option<CliError>), CliError> {
    let cred = plan.credential;
    let pair = cred.pair();
    let range = plan.range();

    // The demographics: before the header, so a refused key writes nothing.
    let patients = api.resources(
        DEMOGRAPHICS.endpoint,
        &pair,
        &cred.patient_id,
        &Range::Complete,
        &DEMOGRAPHICS.extra(),
    )?;
    let patient_ref = patients
        .iter()
        .find(|r| r.resource_type == "Patient" && !r.id.is_empty())
        .map(|r| r.id.clone());

    let mut header = Header::new(
        Source::new(crate::commands::BIN, plan.version),
        plan.organization.clone(),
        plan.patient.clone(),
        plan.pulled_at.clone(),
    );
    header.fhir_version = FHIR_VERSION.into();
    if let Some(s) = plan.since {
        header.since = Some(pk_cli_core::dates::fmt_iso(s));
        header.since_unbounded = vec!["Patient".into()];
    }
    let mut w = RecordsWriter::start(out, &header)?.max_line_bytes(plan.max_line_bytes);
    // A resource ICANotes sends without an id (its CareTeam, in the
    // published samples) goes in as sent: the stream always writes one, and
    // medrec names it by its content.
    for r in &patients {
        w.resource(r)?;
    }

    let finish = |w: RecordsWriter<W>, e: Option<CliError>| {
        let done = w.finish(
            pk_cli_core::dates::fmt_rfc3339(crate::ratelimit::now()),
            e.as_ref(),
        )?;
        Ok((done, e))
    };

    for (i, s) in SEARCHES.iter().enumerate() {
        note(&format!("{} ({} of {})", s.endpoint, i + 2, REQUESTS));
        match api.resources(s.endpoint, &pair, &cred.patient_id, &range, &s.extra()) {
            Ok(rs) => {
                for r in &rs {
                    w.resource(r)?;
                }
            }
            Err(e) if ends_export(&e) => {
                return finish(w, Some(e));
            }
            Err(e) => w.skip(skip_for(&s.label(&range), &e)),
        }
    }

    note(&format!("{CCD_ENDPOINT} ({REQUESTS} of {REQUESTS})"));
    let ccd_label = format!(
        "{CCD_ENDPOINT}?{}",
        match range {
            Range::Complete => "req_type=Complete",
            Range::Between { .. } => "req_type=DateRange",
        }
    );
    match api.ccd(&pair, &cred.patient_id, &range) {
        Ok((zip, count)) => {
            match crate::zip::read(&zip).map(|entries| ccd::documents(entries, &cred.patient_id)) {
                Ok(archive) => {
                    for why in &archive.unreadable {
                        w.skip(Skip::search(
                            ccd_label.clone(),
                            skip::UPSTREAM,
                            format!("a file of the archive is not a C-CDA document: {why}"),
                        ));
                    }
                    let docs = archive.documents;
                    if let Some(c) = count {
                        if c as usize != docs.len() {
                            w.skip(Skip::search(
                                ccd_label.clone(),
                                skip::NO_FILE,
                                format!(
                                    "ICANotes said the archive holds {c} documents and it holds {}",
                                    docs.len()
                                ),
                            ));
                        }
                    }
                    for d in &docs {
                        let binary = ccd::raw(&ccd::binary(d))?;
                        if binary.raw.len()
                            > plan.max_line_bytes.min(pk_med_records::MAX_LINE_BYTES)
                        {
                            // Its DocumentReference would name a Binary the
                            // stream cannot carry; `documents download` can.
                            w.skip(Skip::resource(
                            "DocumentReference",
                            &d.id,
                            skip::TOO_LARGE,
                            format!(
                                "the document is {} bytes, over what one stream line carries as base64; `icanotes documents download --profile <NAME> {} -o <FILE>` saves it",
                                d.bytes.len(),
                                d.id
                            ),
                        ));
                            continue;
                        }
                        w.resource(&ccd::raw(&ccd::document_reference(
                            d,
                            patient_ref.as_deref(),
                        ))?)?;
                        w.resource(&binary)?;
                    }
                }
                Err(e) => w.skip(skip_for(&ccd_label, &e)),
            }
        }
        Err(e) if ends_export(&e) => return finish(w, Some(e)),
        Err(e) => w.skip(skip_for(&ccd_label, &e)),
    }
    finish(w, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_carry_no_patient_or_key() {
        let s = SEARCHES.iter().find(|s| s.endpoint == "Condition").unwrap();
        assert_eq!(
            s.label(&Range::Complete),
            "Condition?req_type=Complete&cat_type=All"
        );
        assert_eq!(REQUESTS, 13);
    }
}
