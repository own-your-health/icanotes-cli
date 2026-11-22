# Privacy

`icanotes` is an open-source command-line program a patient runs on their own
computer to download their own records from a practice that uses ICANotes.
This page is its privacy policy, and the rule its repository is kept by.

## What icanotes reads

With the API key and secret the patient generates on their practice's patient
portal (patientonlineportal.com), icanotes reads through ICANotes' patient
API:

- the patient's demographics, allergies, medication requests, conditions,
  observations, diagnostic reports, immunizations, procedures, devices, care
  plans, goals and care team;
- the C-CDA documents ICANotes produces for the patient.

A key answers for one patient only. icanotes reads nothing else, and it never
changes anything at ICANotes.

## Where the data goes

icanotes writes it only to the patient's own computer.

- icanotes sends no record, and nothing about one, to its authors or to any
  other party. It has no server, no analytics, no telemetry, no crash
  reporting and no advertising.
- What icanotes downloads goes to the patient's standard output, normally
  straight into another program the patient runs, such as `medrec import`,
  or, for `documents download`, to files the patient names. Where that
  program or those files keep records is the patient's choice, not
  icanotes'.
- The API key and secret are kept in icanotes' vault: files under
  `~/.config/icanotes/vault`, encrypted with a key kept in the operating
  system's credential store (the macOS Keychain), readable only by the
  patient's account. They are never printed or logged, and never written to a
  file unencrypted.
- icanotes keeps a settings file, readable only by the patient, under
  `~/.config/icanotes`: its profiles, one per portal account, and for each the
  patients it holds keys for (each one's label and ICANotes patient id). It
  also keeps `rate.json` there, the state of ICANotes' request window. Neither
  holds a record.
- Each profile signs in to the portal in its own Chrome profile (on macOS
  under `~/Library/Application Support/piekstra.icanotes`), which keeps that
  portal account's cookies until the profile is removed.

icanotes connects to ICANotes' API servers and the patient portal, and to
GitHub only when the patient asks it to update itself.

## Stopping access

`icanotes auth logout --profile <NAME>` removes the keys that profile holds.
`icanotes profiles remove <NAME>` also removes its settings and its Chrome
profile. `icanotes auth logout --profile <NAME> --forget`, once no profile
holds a key, also deletes the vault's key from the Keychain, leaving icanotes
nothing there. A key stays valid at ICANotes until the patient deletes it on the
portal's FHIR API Access page; doing so ends icanotes' access.

## Contact

Questions about this policy: open an issue at
https://github.com/own-your-health/icanotes-cli/issues. Never put a record, a
patient id or any personal detail in an issue.

## The repository carries no personal data

Everything committed here is public. It holds:

- no record, response, document or export from a real patient, under any
  extension (`.gitignore` blocks `*.ndjson`, `*.har`, `*.zip`, `*.xml` and
  capture directories);
- no name, patient id, address, phone number, email address or date
  belonging to a real person;
- no API key, secret, token or cookie.

Test data is invented, with obviously fake values. `tests/no_personal_data.rs`
scans every tracked file for the shapes personal data takes, and CI runs
gitleaks on every push and pull request.
