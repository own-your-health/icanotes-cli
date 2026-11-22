# AGENTS.md — icanotes-cli

`icanotes` reads a patient's own records from a practice on ICANotes, a
behavioral-health EHR, through the patient API key the practice's portal
(patientonlineportal.com) issues. It conforms to piekstra-cli/1 and
medical-common's `health-records/v1` and `documents/v1` profiles, on the
`pk-cli-*` crates from [cli-common](https://github.com/piekstra/cli-common)
and the `pk-med-*` crates from
[medical-common](https://github.com/own-your-health/medical-common).

## Layout

| File | Owns |
|---|---|
| `src/main.rs` | dispatch, `info` (with `setup`), exit codes, `--profile` refused where it means nothing |
| `src/cli.rs` | the clap tree |
| `src/commands/*.rs` | one module per command group; `Ctx` in `mod.rs`: the config, `--profile`, the vault (or the test service's), the transport, the browser window, `rate.json` |
| `src/config.rs` | the config (`api_base`, `portal_url`), the profiles and each one's grants (`ProfileSettings`, `Grant`), the organization, which hosts are allowed (checked on every load) |
| `src/credential.rs` | the key and secret: parsing `<key>:<secret>`, the vault entry `<profile>/icanotes/<patient id>` |
| `src/client.rs` | the patient API: requests, statuses, parsing answers; `Fetch` is the transport seam |
| `src/redact.rs` | removing the key and secret from text, bytes and errors: in place where written or percent-encoded, and any token that decodes to one (JSON escapes, HTML references, mixed or double encoding) |
| `src/ratelimit.rs` | ICANotes' window of 10 and the 15-minute block after a 429, kept between runs and shared by runs at once (`rate.json` under `rate.lock`) |
| `src/export.rs` | what `records export` pulls, in order, which failures skip and which end it |
| `src/zip.rs` | the minimal ZIP reader for `PatientCCD` |
| `src/ccd.rs` | C-CDA headers, document ids and file names, the DocumentReference and Binary |
| `src/login.rs` | `auth login --browser`: the expression that reads the key off the page (`make check-key-expression`), remembered verdicts |
| `tests/support/mock.rs` | the mock ICANotes API on 127.0.0.1, shared by `tests/e2e.rs` and `examples/mock_icanotes.rs` (`make mock`) |
| `docs/api.md` | ICANotes' API, its sources and its traps; read before changing the client, export or login |
| `docs/registration.md` | setup for anyone: the practice's portal account, generating the key, storing it, what each refusal means |

## Medical data

These are behavioral-health records: the most sensitive a household has.

- The repository carries no personal data: invented test data only, checked
  by `tests/no_personal_data.rs` and gitleaks. Never commit ICANotes'
  documentation or its samples (its terms keep the documentation unshared);
  `docs/api.md` states facts in its own words.
- Develop and dogfood against the mock (`make mock`). A check against real
  data reports counts, statuses and exit codes only. Real records never go
  into tracked files, commits, PRs, issues or messages to other sessions.
- `records export -o` is refused unless `api_base` is a test server (the mock,
  or ICANotes' `r2test` host): a patient's own records go to stdout, which
  `medrec import --from icanotes` reads.
- No MCP surface: no `mcp` command and no cli-mcp registration.

## Credentials

- Profiles: one per patient-portal account, in the config only. Every
  command that uses a key (`auth`, `patients`, `records`, `documents`, `api`)
  needs `--profile <NAME>` or `ICANOTES_PROFILE`; there is no default, and a
  lone profile is never picked (exit 2, listing them). Every other command
  refuses `--profile`.
- The key and secret travel **in the query string**. Never log a request URL
  or put one in an error; `Reqwest` strips URLs from transport errors, and
  every error from a request passes through the pair's `Redactor`. `api`
  refuses `userKey`/`userSecret` in its path and redacts its output.
- Every key is in icanotes' vault (cli-common `pk_cli_auth::vault`): files
  under `~/.config/icanotes/vault/`, encrypted with one Keychain item in
  service `piekstra.icanotes`. A newly built binary asks macOS once. Install
  and upgrade only when the owner is present.
- `auth login --browser` opens Chrome in the profile's own browser profile
  (`browser-profile-<profile>`) through `pk_cli_browser::capture`. icanotes
  never touches the sign-in form. A session working for the owner may fill
  it from 1Password over stdin (`op read … | browser.mjs fill --stdin`), on
  the debug port in that profile's `DevToolsActivePort`; it checks the fields
  are non-empty before submitting, and the owner clicks any consent, terms or
  code prompt.
- Setup is explained in `docs/registration.md`, at `commands::SETUP_DOC`.
  `info` reports `setup` (the doc; `configured`, true when at least one
  profile records a key; `logins`, each profile's `keys`, or its `error`
  when its settings do not read) from the config file alone, never the
  vault, so it runs anywhere and never prompts. `auth status` carries `setup_doc`, and the missing-key error names
  the doc with the command to run.
- `KEY_EXPRESSION` answers `{"stopped": "portal-disabled"}` when the sign-in
  says the practice turned the portal off, and the confirm hook ends the wait
  with `Confirm::Fail` (exit 3). The capture reads only https pages on the
  default port, so no local page can stand in for the portal: the
  expression's cases run in Chromium through `make check-key-expression`.
- Tests and `make verify` never read the real Keychain:
  `ICANOTES_KEYCHAIN_SERVICE` names an empty service for the surface tests
  and `smoke`, with the vault beside their scratch config; the in-process
  tests use `MemoryKeys`.
- If an `icanotes` call hangs, it waits on a Keychain or 1Password approval.
  Stop calling it and tell the owner; a retry only raises another dialog.

## Conventions

- `--json` on every command, one schema-tagged document on stdout, except
  `records export --ndjson`, whose stream is the output.
- Arguments, and secrets from `--stdin` or `--from-env`, are checked before
  the Keychain or the network.
- Read-only: icanotes never writes to ICANotes; `api` takes GET only.
- `api_base` is https on `icanotes.com` or a host under it, or a loopback
  test server; `portal_url` must be https. Both are checked when the config
  loads, so a hand-edited file or one named by `--config` cannot send the
  stored key elsewhere; only `config show|set|unset` read it unchecked.
- A resource ICANotes sends without an id goes into the stream as sent;
  icanotes never makes one up.
- Every binary-producing `make` target signs with the stable
  `pk-cli-codesign` identity.

## Tests

`make verify` (fmt, clippy, tests, smoke), offline:

- `tests/cli_surface.rs`: help, exit codes, JSON errors, `info`,
  completions, `orgs`, `config`, `profiles`, `--profile` required and
  refused, a profile with nothing stored, malformed pairs never echoed, a
  hand-edited `api_base` refused, the setup doc named by `info`, `auth
  status` and the missing-key error, and `info` with a config that does not
  read.
- `tests/e2e.rs`: in process against the mock, as profile `adult1`: browser
  login (one check per pair shown), a refused pair never stored,
  `set-credential`, a full export (counts, ids, the Binary's bytes, 13 keyed
  requests), `--since`, failing endpoints as skips, empty and null ids
  passed through, a document too large for a line skipped whole, a 401 mid-export, a 401 before the header (nothing
  written), a 429 and the block kept for the next run, waiting out a window
  of 10, a window longer than `--max-wait`, documents download, `api`
  redaction (encoded and JSON-escaped echoes), a transport error naming no
  URL, status, logout and profile removal.
- `tests/fixture_shapes.rs`: the fixtures read as the client reads them, keep
  the shapes the export handles, and carry only dummy identities.
- `tests/no_personal_data.rs`: personal-data shapes in tracked files.
- Unit tests in each module.
- `scripts/check-key-expression.mjs` (`make check-key-expression`, needs
  Playwright): the login page script against synthetic portal pages.

## Definition of done

`make verify` and CI green, the installed binary dogfooded against the mock,
`docs/api.md` matching what ICANotes does, and no personal data anywhere in
the diff.
