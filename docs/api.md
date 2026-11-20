# ICANotes' patient API

What `icanotes` relies on, where each fact comes from, and the traps. Read it
before changing the client, the export or the login.

## Sources

- ICANotes' API documentation, published as a Postman collection:
  https://documenter.getpostman.com/view/2843466/RW1bozWC (published
  2018-09-13). Its terms ask developers not to share copies of the
  documentation, so this page states what icanotes depends on in its own words
  and quotes a sentence at most; read the original for the rest.
- Requests made by icanotes' author: one unauthenticated request to
  `r2.icanotes.com` (its status and headers below), and the patient portal's
  sign-in page at https://patientonlineportal.com/.
- What a signed-in patient sees: see [Signed-in evidence](#signed-in-evidence).

## Route

A practice on ICANotes, a behavioral-health EHR, gives its patients a portal
at `patientonlineportal.com`. A signed-in patient generates API credentials
there, and ICANotes' patient API at `https://r2.icanotes.com/API` answers for
that patient's own chart with them. There is no OAuth, no app registration and
no consent screen: the key and secret are the patient's.

- The portal's sign-in is a WS-Federation form on the same host
  (`/idp/account/signin`, fields `UserName` and `Password`, a "Remember me?"
  checkbox). icanotes never fills it; the patient does, or a session working
  for the patient fills it from 1Password over stdin.
- A practice can turn the portal off for a login. The sign-in then stays on
  `/idp/account/signin` and shows "Use of the Patient Portal has been
  disabled. Please contact your provider with questions." with a Return to
  Login button, in place of an invalid-password message. No key can be
  generated until the practice turns it back on.
- The docs place the credentials on the portal: log in, then request them.
  The page is My History, FHIR API Access, Generate New Credentials.
- `r2test.icanotes.com` is the docs' test host. It serves the docs' sample
  patient; icanotes treats it as a test server (`records export -o` is
  allowed against it).

## Credentials

- A pair: `userKey` and `userSecret`. Every request carries both **as query
  parameters** (`?userKey=…&userSecret=…`). So a request URL is a credential:
  icanotes never logs one, strips URLs from transport errors
  (`reqwest::Error::without_url`), and redacts both values from every error
  and from `api` output (`src/redact.rs`): as written and percent-encoded in
  place, and in any other spelling an upstream may echo them in (lowercase
  or partial percent-encoding, doubly encoded, JSON-escaped like
  `\/` and `\u002B`, HTML character references) by replacing the whole
  token that decodes to one.
- They do not expire. The docs: "userTokens do not get deleted unless you
  delete them from the Patient Portal page yourself."
- Generating does not replace earlier pairs. The docs: there is "no need to
  create multiple userTokens to send out requests, but you do have the
  ability to generate as many as needed", and each can be regenerated or
  deleted on the portal. So `auth logout` and `profiles remove` only forget
  icanotes' copy; the pair stays valid at ICANotes until deleted there, and
  both commands say so.
- The secret is shown once, on the page that generated it. `auth login
  --browser` reads it off that page; a pair copied by hand goes through
  `auth set-credential` (stdin, `--from-env` or a prompt that does not echo).
- A pair belongs to one patient. `PatientID` (below) gives that patient's id;
  icanotes stores the pair with it, as the vault entry
  `<profile>/icanotes/<patient id>`.

## Requests

`GET https://r2.icanotes.com/API/<Endpoint>?userKey=&userSecret=&patientID=&req_type=…`

| Endpoint | FHIR type | `req_type` | Own parameters |
|---|---|---|---|
| `PatientID` | (none) | (none; no `patientID`) | |
| `PatientDemographics` | Patient | `Complete` only | |
| `AllergyIntolerance` | AllergyIntolerance | `Complete`, `DateRange`, `SpecificDate` | |
| `MedicationRequest` | MedicationRequest | same | |
| `Condition` | Condition | same | `cat_type` (`All`, or one category) |
| `Observation` | Observation | same | `cat_type` (`All`, or one category) |
| `DiagnosticReport` | DiagnosticReport | same | |
| `Immunization` | Immunization | same | |
| `Procedure` | Procedure | same | |
| `Device` | Device | same | |
| `CarePlan` | CarePlan | same | |
| `Goal` | Goal | same | |
| `CareTeam` | CareTeam | same | |
| `PatientCCD` | C-CDA documents | `Complete`, `DateRange` | |

- `DateRange` takes `FromDate` and `ToDate` as `MM/DD/YYYY`; `SpecificDate`
  takes `Date`. icanotes uses `Complete`, or `DateRange` from `--since` to
  today. A type's date is its own: for allergies, the docs say, the date the
  allergy was entered into the EHR, "and may not correspond to the actual
  date the allergy physically manifested". So `--since` can leave out an
  older allergy entered long ago; a full export has none of that doubt.
- `PatientDemographics` has no date range, so `--since` exports the patient
  whole and the stream header says so (`since_unbounded: ["Patient"]`).
- icanotes sends GET only, follows no redirect, and accepts at most 256 MiB
  per answer.

## Answers

- A resource endpoint answers a JSON array of entries,
  `[{"resource": {…}, "request": {"method": "POST"}}]`. The client also reads
  a bare array of resources, a Bundle, a single resource and an empty body
  (none on file). Any other object is an error naming its keys only.
- The resources are shaped on FHIR STU3, not R4: `clinicalStatus` and
  `verificationStatus` are codes rather than CodeableConcepts,
  `MedicationRequest.requester.agent`, `Immunization.notGiven`,
  `DiagnosticReport.category` a single CodeableConcept. icanotes passes them
  through unchanged and declares `fhir_version: "3.0.1"` in the stream header.
- Empty strings stand for absent values in places
  (`MedicationRequest.dispenseRequest.validityPeriod.start: ""`). Passed
  through unchanged.
- The docs' CareTeam sample has no `id`, and empty strings stand in for
  absent values, so an id may be missing, `""` or `null`. icanotes passes
  such a resource through as sent; the stream always writes a resource
  without an id, and medrec (0.2.7 and later) names it by its content.
  icanotes gives none an id of its own: one numbered by answer order would
  move to another resource when the order changes.
- Resource ids look like `MU2014-<number>`, `MU2014-<GUID>` or
  `MU2014-<number>-<suffix>`; the Patient's is `MU2014-<patient id>`, and
  every `subject` or `patient` reference names it.
- `PatientID` answers `[{"patientID": "<digits>"}]`. icanotes uses this id
  (what every other request takes) as the stream's patient id, not the
  Patient resource's `MU2014-…` id.
- `PatientCCD` answers a ZIP of C-CDA XML files ("streamed-bytes"), with
  headers `documentCount`, `fileFormat: Zip` and `patientID`. ICANotes
  publishes a stylesheet for them at https://www.icanotes.com/api/CDA.xsl.
  icanotes reads the ZIP itself (`src/zip.rs`: stored and deflated entries,
  CRC-checked, no ZIP64, encryption or multi-disk, at most 500 entries and
  64 MiB each), keeps the `.xml` entries that read as a ClinicalDocument,
  and checks `documentCount` against them (a mismatch is a skip, not a
  failure). An `.xml` entry that is not one (an IHE XDM `METADATA.XML`)
  is a skip, and the others are kept.
- A stream line holds at most 64 MiB, so a document over about 48 MiB
  cannot travel as a base64 Binary. The export skips it whole
  (`too_large`, naming its id), DocumentReference included, so nothing
  points at a missing Binary; `documents download <id> -o <FILE>` saves it.

## Statuses

| Status | The docs | icanotes |
|---|---|---|
| 200 | success | |
| 400 | a malformed or missing parameter | exit 5; a skip during an export |
| 401 | "Unauthorized (Invalid AuthToken)" | exit 3: the pair was deleted or replaced. Ends an export |
| 403 | not enough permission, or "the request to the API was sent incorrectly" | exit 3; a skip during an export. An unauthenticated request gets 403 too |
| 404 | (not documented) | exit 4; a skip |
| 429 | too many requests | exit 5. Ends an export, and nothing is sent for 15 minutes |
| 500 | a server error | exit 5; a skip |

Error bodies are never printed: they may quote the request.

## Rate limit

- The docs set no overall limit but answer "abuse" with 429, after which a
  client must "wait 15 minutes before attempting to try their access again".
- Every answer carries `X-RateLimit-Limit: 10` and `X-RateLimit-Remaining`
  (the docs' samples count down from 9). The unauthenticated request also
  carried `X-RateLimit-Reset`, in Unix seconds, about ten minutes ahead.
- An export makes 13 requests (`PatientDemographics`, the 11 searches,
  `PatientCCD`), so it waits out one window. icanotes keeps the window in
  `rate.json` beside the config, read and written under a lock on
  `rate.lock`, so one run's requests count against the next and two runs at
  once (two profiles' exports, an export and `auth status --check`) share
  one window. Each request takes its slot before it is sent, and an answer
  counted in the same window (a reset within a minute of the stored one)
  never raises the count another run lowered; one counted in the window
  before, arriving late, is ignored. A `rate.json` that does not read is
  refused rather than taken as a fresh window, which could forget a 429. When
  `Remaining` is 0 it waits until the reset (and says so on stderr), and
  refuses instead when the wait is longer than `--max-wait` (default 16
  minutes). When an answer says the window is used up without a reset
  time, it takes the window to last 11 minutes. After a 429 it sends
  nothing until 15 minutes have passed, whichever run met it. medrec's
  import timeout (30 minutes) covers one wait.
- `auth login --browser` checks a pair once (one `PatientID` request) and
  remembers the answer while the page keeps showing it.

## The C-CDA as FHIR

Each C-CDA in the ZIP becomes a DocumentReference and a Binary in the
stream, as medrec expects of documents:

- The id is `ccd-` and 24 hex digits of SHA-256 over the patient id, the
  document's code, effective time and title (the entry's name when the
  header has none), so the same document has the same id on every pull.
  `documents list|download` use the same ids.
- The DocumentReference: `meta.source: icanotes:ccd` and the tag
  `mapped-from-ccd` (system `https://github.com/piekstra/icanotes-cli/fhir/tag`),
  `status: current`, the LOINC `type` when the header's code is LOINC,
  the us-core `clinical-note` category, `subject` the Patient, `date` only
  when the effective time has a time of day and a zone, and one attachment:
  `contentType: application/xml`, `url: Binary/<id>`, size, title and
  creation date.
- The Binary carries the file's bytes unchanged, base64.
- `documents download` names a file by the day, the title and the last 8
  characters of the id (`2020-01-02_Continuity_of_Care_Document_861989cb.xml`),
  so two of one day and title never share a file and a document keeps its
  name on every download.
- icanotes reads only the header (code, title, effective time), stopping at
  the patient, author, custodian or body; its XML errors quote nothing.

## Signed-in evidence

Not yet confirmed against a signed-in account. The facts above come from the
published docs and one unauthenticated request. To be recorded here after the
first real `auth login --browser`, as shapes and counts only:

- the FHIR API Access page: where the key and secret appear, and whether
  generating asks to accept terms;
- whether generating a pair leaves earlier ones valid;
- what each endpoint answers for a patient with nothing on file (an empty
  array or an empty body), and whether production shapes match the docs'
  samples (STU3 fields, id-less CareTeam, `documentCount`).

## Traps

- **The key is in the query.** Never log, print or put a request URL in an
  error. `api` refuses `userKey` and `userSecret` in its path, adds them
  itself, and redacts them from what it prints or writes.
- **403 means two things.** A pair for another patient, and a request with no
  pair at all, both answer 403; a deleted pair answers 401.
- **13 requests, 10 allowed.** A naive client gets a 429 and fifteen minutes
  locked out on its first full pull.
- **Dates are per type, and `MM/DD/YYYY`.** `--since` is a day ICANotes
  dates each type by, not the clinical date.
- **STU3, not R4.** A consumer reading R4 shapes finds `clinicalStatus` a
  string.
- **No id on CareTeam.** An id assigned by answer order would give one
  team's id to another when the order changes, and a store would overwrite
  one with the other. icanotes leaves it out; the store names it by content.
- **A turned-off portal looks like a sign-in still to finish.** The page is
  the sign-in page, so a wait for the key would run to its timeout.
  `auth login` reads the notice and ends at once with exit 3, naming the
  practice as the remedy.
- **An answer can echo the query.** An error body or a passthrough answer
  may repeat the key and secret in any encoding; `api` output passes
  through the redactor, pretty-printed output a second time.
