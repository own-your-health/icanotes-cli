# Changelog

## v0.1.1 (2026-11-19)

- `auth login --browser` ends at once with exit 3 when the portal's sign-in
  says "Use of the Patient Portal has been disabled", instead of waiting out
  `--timeout` on a sign-in that cannot finish. The remedy is to ask the
  practice to turn the portal back on.

## v0.1.0 (2026-11-19)

First release.

- `auth login --browser`: opens the patient portal in the profile's own
  Chrome profile, reads the key and secret the FHIR API Access page
  generates, checks them with one `PatientID` request and stores them in the
  vault. `auth set-credential` stores a pair from stdin, `--from-env` or a
  prompt. `auth status [--check]`, `auth logout [--forget]`.
- Profiles, one per portal account; `--profile` or `ICANOTES_PROFILE`, no
  default.
- `records export`: demographics, the 11 resource endpoints and the C-CDA
  documents (as DocumentReference and Binary) as `records-stream/v1`, FHIR
  STU3 as ICANotes serves it and passed through unchanged; `--since` as a
  date range. `-o` only for a test server.
- `documents list|download` for the C-CDA files (`documents/v1`).
- `api GET`: one request, the key and secret added by icanotes and redacted
  from the output in any encoding the answer echoes them in.
- ICANotes' request window of 10 kept in `rate.json` and shared by every
  run, one at a time or at once: an export waits once (`--max-wait`), and
  nothing is sent for 15 minutes after a 429.
- `orgs`, `config`, `self-update`, `completions`, `info` (`health-records/v1`,
  `documents/v1`, `login-profiles`).
