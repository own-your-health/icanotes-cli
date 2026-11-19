//! The patient's C-CDA documents (`PatientCCD`, a ZIP of XML files) as FHIR
//! `DocumentReference` + `Binary` pairs, the shape `medrec` files a
//! document from, and as `documents/v1` rows.
//!
//! The XML is kept byte for byte in the Binary. The DocumentReference is
//! icanotes' own mapping of the document's header (its type code, title and
//! time), marked with `meta.source` and a `meta.tag`, and its id is derived
//! from what identifies the document across pulls, so the same document
//! keeps one id on every export.

use base64::Engine;
use pk_cli_core::CliError;
use pk_med_fhir::RawResource;
use quick_xml::events::Event;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::zip::ZipEntry;

pub const META_SOURCE: &str = "icanotes:ccd";
pub const TAG_SYSTEM: &str = "https://github.com/piekstra/icanotes-cli/fhir/tag";
pub const TAG_MAPPED: &str = "mapped-from-ccd";
pub const CONTENT_TYPE: &str = "application/xml";
const LOINC: &str = "http://loinc.org";
const LOINC_OID: &str = "2.16.840.1.113883.6.1";

/// What a C-CDA's header says about it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CdaHeader {
    /// `ClinicalDocument/code`: the document type, LOINC in a C-CDA.
    pub code: Option<String>,
    pub code_system: Option<String>,
    pub code_display: Option<String>,
    pub title: Option<String>,
    /// `ClinicalDocument/effectiveTime/@value`, an HL7 TS
    /// (`20200102031500+0500`).
    pub effective: Option<String>,
}

/// One document from the archive.
#[derive(Clone)]
pub struct CcdDocument {
    pub id: String,
    /// Its name in the archive.
    pub entry: String,
    pub header: CdaHeader,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for CcdDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CcdDocument")
            .field("id", &self.id)
            .field("bytes", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// Read the header elements directly under `ClinicalDocument`, stopping at
/// the first participant or the body: nothing about the patient is read.
pub fn header(xml: &[u8]) -> Result<CdaHeader, CliError> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut out = CdaHeader::default();
    let mut depth = 0usize;
    let mut in_title = false;
    let mut root_seen = false;
    let mut buf = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buf).map_err(|e| {
            CliError::Upstream(format!(
                "a C-CDA document does not read as XML at byte {}: {}",
                reader.buffer_position(),
                xml_error_kind(&e)
            ))
        })?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(event, Event::Empty(_));
                let name = e.local_name();
                let name = name.as_ref();
                if depth == 0 {
                    if name != b"ClinicalDocument" {
                        return Err(CliError::Upstream(
                            "an XML file in the C-CDA archive is not a ClinicalDocument".into(),
                        ));
                    }
                    root_seen = true;
                } else if depth == 1 {
                    match name {
                        b"recordTarget" | b"author" | b"component" | b"custodian" => break,
                        b"code" => {
                            out.code = attr(e, b"code");
                            out.code_system = attr(e, b"codeSystem");
                            out.code_display = attr(e, b"displayName");
                        }
                        b"effectiveTime" => out.effective = attr(e, b"value"),
                        b"title" if !empty => in_title = true,
                        _ => {}
                    }
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::Text(t) if in_title => {
                let text = t.unescape().map(|c| c.into_owned()).unwrap_or_default();
                let joined = match out.title.take() {
                    Some(prev) => format!("{prev} {text}"),
                    None => text,
                };
                out.title = Some(joined.trim().to_string()).filter(|s| !s.is_empty());
            }
            Event::End(_) => {
                in_title = false;
                depth = depth.saturating_sub(1);
                if depth == 0 && root_seen {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if !root_seen {
        return Err(CliError::Upstream(
            "an XML file in the C-CDA archive has no ClinicalDocument".into(),
        ));
    }
    Ok(out)
}

/// quick-xml's error without the text it quotes.
fn xml_error_kind(e: &quick_xml::Error) -> &'static str {
    match e {
        quick_xml::Error::Io(_) => "a read error",
        quick_xml::Error::Syntax(_) => "a syntax error",
        quick_xml::Error::IllFormed(_) => "ill-formed XML",
        quick_xml::Error::InvalidAttr(_) => "an invalid attribute",
        quick_xml::Error::Encoding(_) => "an encoding error",
        quick_xml::Error::Escape(_) => "an invalid escape",
        quick_xml::Error::Namespace(_) => "a namespace error",
    }
}

fn attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == key)
        .and_then(|a| a.unescape_value().ok().map(|v| v.trim().to_string()))
        .filter(|v| !v.is_empty())
}

/// What an archive holds.
#[derive(Debug, Default)]
pub struct Archive {
    /// Its C-CDA documents, each with its stable id.
    pub documents: Vec<CcdDocument>,
    /// Files that are not `.xml` (a stylesheet): counted, not kept.
    pub other: usize,
    /// For each `.xml` file that did not read as a C-CDA document (an IHE
    /// XDM `METADATA.XML`, a file in another encoding), why, naming
    /// nothing in it. The others are kept.
    pub unreadable: Vec<String>,
}

/// The documents in an archive's entries.
pub fn documents(entries: Vec<ZipEntry>, patient_id: &str) -> Archive {
    let mut out = Archive::default();
    for e in entries {
        if !e.name.to_ascii_lowercase().ends_with(".xml") {
            out.other += 1;
            continue;
        }
        let header = match header(&e.bytes) {
            Ok(h) => h,
            Err(err) => {
                out.unreadable.push(err.message().to_string());
                continue;
            }
        };
        let mut id = document_id(patient_id, &header, &e.name);
        // Two documents the same in type, time and title: told apart by
        // their order in the archive.
        let mut n = 1;
        while out.documents.iter().any(|d| d.id == id) {
            n += 1;
            id = format!("{}-{n}", document_id(patient_id, &header, &e.name));
        }
        out.documents.push(CcdDocument {
            id,
            entry: e.name,
            header,
            bytes: e.bytes,
        });
    }
    out
}

/// `ccd-` and 24 hex digits of a SHA-256 over the patient and what makes
/// the document itself: its type, time and title when it states a time,
/// else its name in the archive.
pub fn document_id(patient_id: &str, h: &CdaHeader, entry: &str) -> String {
    let key = match &h.effective {
        Some(t) => format!(
            "{}|{}|{}",
            h.code.as_deref().unwrap_or(""),
            t,
            h.title.as_deref().unwrap_or("")
        ),
        None => format!("entry:{entry}"),
    };
    let digest = Sha256::digest(format!("{patient_id}|{key}").as_bytes());
    let hex: String = digest.iter().take(12).map(|b| format!("{b:02x}")).collect();
    format!("ccd-{hex}")
}

/// An HL7 TS as a FHIR dateTime: `20200102` is `2020-01-02`,
/// `20200102031500+0500` is `2020-01-02T03:15:00+05:00`. A time with no
/// zone is cut to its day, which FHIR's dateTime allows.
pub fn fhir_date_time(ts: &str) -> Option<String> {
    let (digits, zone) = match ts.find(['+', '-']) {
        Some(i) => (&ts[..i], Some(&ts[i..])),
        None => (ts, None),
    };
    let digits = digits.split('.').next().unwrap_or("");
    if digits.len() < 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let part = |a: usize, b: usize| digits.get(a..b);
    let date = match digits.len() {
        4 => digits.to_string(),
        6 => format!("{}-{}", part(0, 4)?, part(4, 6)?),
        _ => format!("{}-{}-{}", part(0, 4)?, part(4, 6)?, part(6, 8)?),
    };
    if digits.len() < 12 {
        return Some(date);
    }
    let zone = match zone {
        Some(z) if z.len() == 5 && z[1..].bytes().all(|b| b.is_ascii_digit()) => {
            format!("{}:{}", &z[..3], &z[3..])
        }
        _ => return Some(date),
    };
    let secs = part(12, 14).unwrap_or("00");
    Some(format!(
        "{date}T{}:{}:{secs}{zone}",
        part(8, 10)?,
        part(10, 12)?
    ))
}

/// The document's day, `YYYY-MM-DD`, when its time names one.
pub fn day(h: &CdaHeader) -> Option<String> {
    let t = fhir_date_time(h.effective.as_deref()?)?;
    (t.len() >= 10).then(|| t[..10].to_string())
}

/// What the document is called: its title, else its type's name.
pub fn name(doc: &CcdDocument) -> String {
    doc.header
        .title
        .clone()
        .or_else(|| doc.header.code_display.clone())
        .unwrap_or_else(|| "C-CDA document".into())
}

/// The file name `documents download` writes: the day, the name, `.xml`.
pub fn file_name(doc: &CcdDocument) -> String {
    let base = match day(&doc.header) {
        Some(d) => format!("{d}_{}", name(doc)),
        None => name(doc),
    };
    let safe = pk_cli_documents::fs_safe(&base);
    format!("{safe}.xml")
}

/// The file names `documents download --all` writes, one per document:
/// [`file_name`], with the end of the id added to every name two documents
/// share (a day's two notes of one title), so none overwrites another and
/// each keeps its name on the next download.
pub fn file_names(docs: &[CcdDocument]) -> Vec<String> {
    let names: Vec<String> = docs.iter().map(file_name).collect();
    names
        .iter()
        .zip(docs)
        .map(|(n, d)| {
            if names.iter().filter(|m| *m == n).count() < 2 {
                return n.clone();
            }
            let stem = n.strip_suffix(".xml").unwrap_or(n);
            let tail = &d.id[d.id.len().saturating_sub(8)..];
            format!("{stem}_{tail}.xml")
        })
        .collect()
}

fn meta() -> Value {
    json!({
        "source": META_SOURCE,
        "tag": [{ "system": TAG_SYSTEM, "code": TAG_MAPPED, "display": "Mapped by icanotes from a C-CDA document's header" }],
    })
}

/// The document as a DocumentReference of `patient` (a FHIR `Patient.id`,
/// when the export read one).
pub fn document_reference(doc: &CcdDocument, patient: Option<&str>) -> Value {
    let h = &doc.header;
    let mut kind = json!({ "text": name(doc) });
    if let Some(code) = &h.code {
        let loinc = h.code_system.as_deref().is_none_or(|s| s == LOINC_OID);
        if loinc {
            let mut coding = json!({ "system": LOINC, "code": code });
            if let Some(d) = &h.code_display {
                coding["display"] = d.clone().into();
            }
            kind["coding"] = json!([coding]);
        }
    }
    let mut attachment = json!({
        "contentType": CONTENT_TYPE,
        "url": format!("Binary/{}", doc.id),
        "size": doc.bytes.len(),
        "title": file_name(doc),
    });
    let mut v = json!({
        "resourceType": "DocumentReference",
        "id": doc.id,
        "meta": meta(),
        "status": "current",
        "type": kind,
        "category": [{
            "coding": [{
                "system": "http://hl7.org/fhir/us/core/CodeSystem/us-core-documentreference-category",
                "code": "clinical-note",
                "display": "Clinical Note",
            }],
        }],
    });
    if let Some(p) = patient {
        v["subject"] = json!({ "reference": format!("Patient/{p}") });
    }
    if let Some(t) = h.effective.as_deref().and_then(fhir_date_time) {
        if t.contains('T') {
            v["date"] = t.clone().into();
        }
        attachment["creation"] = t.clone().into();
        v["context"] = json!({ "period": { "start": t } });
    }
    v["description"] = format!(
        "C-CDA document from the practice's ICANotes record ({})",
        doc.entry
    )
    .into();
    v["content"] = json!([{ "attachment": attachment }]);
    v
}

pub fn binary(doc: &CcdDocument) -> Value {
    json!({
        "resourceType": "Binary",
        "id": doc.id,
        "meta": meta(),
        "contentType": CONTENT_TYPE,
        "data": base64::engine::general_purpose::STANDARD.encode(&doc.bytes),
    })
}

/// A mapped resource as the stream's raw text.
pub fn raw(v: &Value) -> Result<RawResource, CliError> {
    RawResource::from_value(v)
}

/// The document as a `documents/v1` row.
pub fn document_row(doc: &CcdDocument) -> pk_cli_documents::Document {
    let mut d = pk_cli_documents::Document::new(doc.id.clone(), name(doc));
    d.date = day(&doc.header);
    d.category = Some("C-CDA".into());
    d.file = Some(file_name(doc));
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0"?>
<ClinicalDocument xmlns="urn:hl7-org:v3">
  <realmCode code="US"/>
  <id root="2.16.840.1.113883.19.5" extension="doc-1"/>
  <code code="34133-9" codeSystem="2.16.840.1.113883.6.1" displayName="Summarization of Episode Note"/>
  <title>Continuity of Care &amp; Summary</title>
  <effectiveTime value="20200102030405-0500"/>
  <recordTarget><patientRole><patient><name><given>Synthetic</given></name></patient></patientRole></recordTarget>
</ClinicalDocument>"#;

    #[test]
    fn the_header_is_read_up_to_the_patient() {
        let h = header(DOC.as_bytes()).unwrap();
        assert_eq!(h.code.as_deref(), Some("34133-9"));
        assert_eq!(h.title.as_deref(), Some("Continuity of Care & Summary"));
        assert_eq!(h.effective.as_deref(), Some("20200102030405-0500"));
    }

    #[test]
    fn not_a_clinical_document_is_refused_without_quoting_it() {
        let e = header(b"<html>secret</html>").unwrap_err();
        assert_eq!(e.exit_code(), 5);
        assert!(!e.to_string().contains("secret"));
        assert!(header(b"<ClinicalDocument><title>x</ti></ClinicalDocument>").is_err());
    }

    #[test]
    fn hl7_times_become_fhir_date_times() {
        assert_eq!(fhir_date_time("20200102").as_deref(), Some("2020-01-02"));
        assert_eq!(fhir_date_time("202001").as_deref(), Some("2020-01"));
        assert_eq!(
            fhir_date_time("20200102031500+0500").as_deref(),
            Some("2020-01-02T03:15:00+05:00")
        );
        assert_eq!(
            fhir_date_time("202001020315-0400").as_deref(),
            Some("2020-01-02T03:15:00-04:00")
        );
        assert_eq!(
            fhir_date_time("20200102031500").as_deref(),
            Some("2020-01-02")
        );
        assert_eq!(fhir_date_time("x"), None);
    }

    #[test]
    fn ids_are_stable_and_distinct() {
        let h = header(DOC.as_bytes()).unwrap();
        let a = document_id("1001", &h, "one.xml");
        assert_eq!(a, document_id("1001", &h, "renamed.xml"));
        assert_ne!(a, document_id("1002", &h, "one.xml"));
        assert!(a.starts_with("ccd-") && a.len() == 28 && pk_med_fhir::is_id(&a));
        let none = CdaHeader::default();
        assert_ne!(
            document_id("1001", &none, "a.xml"),
            document_id("1001", &none, "b.xml")
        );
    }

    #[test]
    fn a_document_maps_to_a_reference_and_its_binary() {
        let zip = crate::zip::write(&[("ccd/one.xml", DOC.as_bytes()), ("CDA.xsl", b"<x/>")]);
        let a = documents(crate::zip::read(&zip).unwrap(), "1001");
        assert_eq!((a.documents.len(), a.other), (1, 1));
        let docs = a.documents;
        let d = &docs[0];
        let r = document_reference(d, Some("p-1"));
        assert_eq!(r["type"]["coding"][0]["code"], "34133-9");
        assert_eq!(r["date"], "2020-01-02T03:04:05-05:00");
        assert_eq!(r["subject"]["reference"], "Patient/p-1");
        assert_eq!(
            r["content"][0]["attachment"]["url"],
            format!("Binary/{}", d.id)
        );
        assert_eq!(r["meta"]["tag"][0]["code"], TAG_MAPPED);
        let b = binary(d);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(bytes, DOC.as_bytes());
        assert_eq!(raw(&r).unwrap().resource_type, "DocumentReference");
        let row = document_row(d);
        assert_eq!(row.date.as_deref(), Some("2020-01-02"));
        assert_eq!(
            row.file.as_deref(),
            Some("2020-01-02_Continuity_of_Care___Summary.xml")
        );
    }

    #[test]
    fn twin_documents_get_distinct_ids() {
        let zip = crate::zip::write(&[("a.xml", DOC.as_bytes()), ("b.xml", DOC.as_bytes())]);
        let docs = documents(crate::zip::read(&zip).unwrap(), "1001").documents;
        assert_ne!(docs[0].id, docs[1].id);
        // Each gets its own file, the same on every download.
        let names = file_names(&docs);
        assert_ne!(names[0], names[1]);
        assert_eq!(names, file_names(&docs));
        assert!(
            names[0].ends_with(&format!("_{}.xml", &docs[0].id[20..])),
            "{names:?}"
        );
    }

    #[test]
    fn an_xml_file_that_is_not_a_document_leaves_the_others() {
        let zip = crate::zip::write(&[
            (
                "METADATA.XML",
                b"<SubmitObjectsRequest xmlns=\"urn:x\"/>".as_slice(),
            ),
            ("ccd/one.xml", DOC.as_bytes()),
            (
                "broken.xml",
                b"<ClinicalDocument><title>x</code></ClinicalDocument>".as_slice(),
            ),
        ]);
        let a = documents(crate::zip::read(&zip).unwrap(), "1001");
        assert_eq!(a.documents.len(), 1);
        assert_eq!(a.unreadable.len(), 2, "{:?}", a.unreadable);
    }
}
