# icanotes — your records from a practice on ICANotes

`icanotes` downloads your own records from a practice that uses ICANotes,
through the patient API key the practice's patient portal
(patientonlineportal.com) lets you generate. It writes them as a
`records-stream/v1` stream, the format
[medrec](https://github.com/own-your-health/medrec-cli) imports.

Unofficial; not affiliated with or endorsed by ICANotes.

It reads, and never changes anything at ICANotes:

- demographics, allergies, medication requests, conditions, observations
  (vitals, labs, social history), diagnostic reports, immunizations,
  procedures, devices, care plans, goals and the care team, as FHIR
  resources;
- the C-CDA documents the portal offers, as a DocumentReference and a Binary
  each.

## Who it is for

Any patient of a practice that keeps its records in ICANotes and gives its
patients the ICANotes patient portal. You need a portal account from the
practice; the practice creates it, as the portal has no sign-up of its own.
There is no app to register: you generate a key and secret on the portal, and
icanotes stores them. [`docs/registration.md`](docs/registration.md) walks
through it, including what to do when the portal says "Use of the Patient
Portal has been disabled".

## Status

v0.1. Built and tested against ICANotes' published API documentation and a
local mock serving a synthetic patient. `docs/api.md` records what has and has
not been confirmed on a signed-in account.

## Install

From a release (macOS arm64 and x86_64, Linux x86_64):

```sh
gh release download --repo own-your-health/icanotes-cli --pattern "icanotes-$(rustc -vV | sed -n 's/host: //p').tar.gz"
tar xzf icanotes-*.tar.gz && mv icanotes ~/.local/bin/
```

From source: `make install` (Rust 1.89 or later).

`icanotes self-update` replaces the binary with the latest release.

## Quick start

A profile is one portal login; name it for the person (`me` below). Every
command that uses a key needs `--profile <NAME>` (or `ICANOTES_PROFILE`);
there is no default.

```sh
icanotes profiles add me
icanotes auth login --browser --profile me --label me
icanotes auth status --profile me --check
icanotes records export --profile me --ndjson > me.ndjson
medrec --profile <FOLDER> import --from icanotes --from-profile me --patient me
```

1. `profiles add` names the login.
2. `auth login --browser` opens the portal in Chrome, in that profile's own
   browser profile. Sign in, open My History, then FHIR API Access, and
   choose Generate New Credentials. icanotes reads the key and secret off the
   page, checks them with ICANotes, stores them in its vault, and closes the
   window. Neither is printed.
3. `auth status --check` asks ICANotes whether the stored key works.
4. `records export` writes everything as a `records-stream/v1` stream.
5. [medrec](https://github.com/own-your-health/medrec-cli) runs that export itself
   and files the records.

These steps need a person:
- the portal password, typed or filled by a password manager;
- any code, terms or consent prompt the portal shows;
- on macOS, the Keychain prompt the first time a newly installed icanotes
  stores or reads a key ("Always Allow"; each new build asks once).

The key lasts until you delete it on the portal's FHIR API Access page: it
does not expire. ICANotes' documentation says you can generate as many as
you need, so an earlier key should stay valid when you generate another (not
yet confirmed on a signed-in account). The portal sign-in is needed only
once per key; exports never sign in.
`icanotes auth logout` and `icanotes profiles remove` forget only icanotes'
copy.

A key you generated yourself goes in from stdin instead:

```sh
pbpaste | icanotes auth set-credential --profile me --stdin   # <key>:<secret>
```

`icanotes info` says whether any profile holds a key (`setup.configured`) and
where setup is explained; `icanotes auth status --profile <NAME>` says it for
one profile. A command that finds no key exits 3 and names the command to run
and `docs/registration.md`.

## Use

```sh
icanotes patients list --profile me              # who the profile holds keys for
icanotes records export --profile me --ndjson --since 2026-01-01
icanotes documents list --profile me
icanotes documents download --profile me --all -o ~/Documents/ccd
icanotes api GET 'Goal?req_type=Complete' --profile me
```

ICANotes allows 10 requests per window (about ten minutes) and locks a client
out for 15 minutes after exceeding it. An export makes 13, so it waits once,
saying so on stderr; `--max-wait <MIN>` bounds the wait. Every run shares one
window.

## Commands

| Command | Does |
|---|---|
| `orgs list\|search` | the one organization, ICANotes' patient API (`org-list/v1`) |
| `profiles list\|add\|remove` | the logins (`profile-list/v1`) |
| `auth login --browser` | generate-and-store from the portal |
| `auth set-credential` | store a `<key>:<secret>` from stdin, `--from-env` or a prompt |
| `auth status [--check]` | what is stored, and with `--check` whether ICANotes accepts it (`auth-status/v1`) |
| `auth logout` | forget the stored keys |
| `patients list` | the patients a profile holds keys for (`patient-list/v1`) |
| `records export` | everything, as `records-stream/v1` (`--ndjson` to stdout) |
| `documents list\|download` | the C-CDA documents (`documents/v1`) |
| `api GET <Endpoint>?<query>` | one request, the key added and redacted from the output |
| `config path\|show\|set\|unset` | `api_base`, `portal_url` |
| `self-update`, `completions`, `info` | the family's standard commands; `info` also reports `setup` |

Every command takes `--json` (one schema-tagged document on stdout), except
`records export --ndjson`, whose stream is the output. Exit codes: 0 ok, 2
usage, 3 auth, 4 not found, 5 upstream, 6 confirmation required.

## Build

`make verify` runs formatting, clippy, the tests and a smoke test on the
release binary, offline and without the real Keychain. `CONTRIBUTING.md` shows
how to dogfood against the local mock.

## Privacy

`PRIVACY.md`. In short: records go only to your stdout or files you name;
the key lives in icanotes' vault under the macOS Keychain; nothing goes
anywhere else.

## License

MIT or Apache-2.0, at your option.
