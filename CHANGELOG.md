# Changelog

## v0.1.3 (2026-11-22)

- The repository moved to own-your-health/icanotes-cli. `self-update`,
  `info` and the setup-doc link name the new owner; GitHub redirects the old
  URLs. The FHIR tag system
  (`https://github.com/piekstra/icanotes-cli/fhir/tag`) and the Keychain
  service (`piekstra.icanotes`) are identifiers in stored records and keys,
  and stay as they were.
- medical-common comes from own-your-health/medical-common, at the same tag.

## v0.1.2 (2026-11-20)

- `docs/registration.md`: setup for anyone, from the practice's portal
  account to a stored key, with what each refusal means.
- `info` reports `setup`: the doc, whether any profile holds a key
  (`configured`) and how many keys each profile records. It reads the config
  file only, so it still runs anywhere and never prompts.
- `auth status` carries `setup_doc` and prints a Setup line when the profile
  holds no key; the missing-key error and the disabled-portal error name the
  doc.
- README: who it is for and a quick start from `profiles add` to `medrec
  import`, with the steps that need a person and how long a key lasts.

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
