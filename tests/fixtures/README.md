# Test fixtures

Invented payloads in the shapes ICANotes' patient API answers, served by the
mock server in `tests/support/mock.rs` (and `examples/mock_icanotes.rs`).
Nothing here was captured from ICANotes or a practice, and nothing describes
a real person.

The shapes follow ICANotes' published API samples: a JSON array of
`{"resource": …, "request": {"method": "POST"}}` entries, STU3-style fields
(`clinicalStatus` as a code, `MedicationRequest.requester.agent`,
`Immunization.notGiven`), empty strings where ICANotes sends them
(`dispenseRequest.validityPeriod`), and a CareTeam with no `id`. Every value
is a dummy: the patient is `Alex Testpatient`, born 1900-01-01 (no living person's birth date), at
`100 Example St`, `(555) 010-0001`, `alex@example.com`; ids are short
(`MU2014-SYN-100001`, patient id `100001`); codes are `0000` or public LOINC and
NUCC shapes.

| File | Endpoint it answers |
|---|---|
| `PatientID.json` | `PatientID` |
| `PatientDemographics.json` | `PatientDemographics?req_type=Complete` (pretty-printed, as ICANotes sends some) |
| `AllergyIntolerance.json` | `AllergyIntolerance` |
| `MedicationRequest.json` | `MedicationRequest` |
| `Condition.json` | `Condition?cat_type=All` |
| `Observation.json` | `Observation?cat_type=All` (pretty-printed) |
| `DiagnosticReport.json` | `DiagnosticReport` |
| `Immunization.json` | `Immunization` |
| `Procedure.json` | `Procedure` |
| `Device.json` | `Device`: none on file, `[]` |
| `CarePlan.json` | `CarePlan` |
| `Goal.json` | `Goal` |
| `CareTeam.json` | `CareTeam`: no `id`, as ICANotes' sample |
| `ccd.xml` | The C-CDA inside `PatientCCD`'s ZIP, which the mock builds at run time beside a `CDA.xsl` |

`tests/fixture_shapes.rs` checks each file reads as the client reads it and
that the identity-bearing values are the dummies above.
