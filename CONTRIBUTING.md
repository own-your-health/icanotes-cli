# Contributing

- Read `AGENTS.md` (layout, rules for medical data and credentials) and
  `docs/api.md` (ICANotes' API, its sources and its traps) first.
- `make verify` runs what CI runs: fmt, clippy with `-D warnings`, the tests
  and the smoke test on the release binary. It needs no network beyond the
  first dependency fetch and never reads the real Keychain.
- Dogfood the installed binary against `make mock`, a stand-in for ICANotes'
  patient API serving one synthetic patient. It prints its API base and the
  synthetic key pair it accepts:

  ```sh
  make install && make mock     # in another terminal
  export ICANOTES_KEYCHAIN_SERVICE=piekstra.icanotes.dogfood ICANOTES_PROFILE=dogfood
  icanotes config set api_base <the printed API base>
  icanotes profiles add dogfood
  printf '%s' '<the printed pair>' | icanotes auth set-credential --stdin
  icanotes records export -o /tmp/icanotes-mock
  icanotes documents download --all -o /tmp/icanotes-mock/docs
  icanotes api GET 'Goal?req_type=Complete'
  icanotes auth logout --forget && icanotes profiles remove dogfood --force
  icanotes config unset api_base
  ```

  `cargo run --example mock_icanotes -- --limit 10 --reset 30` serves
  ICANotes' window of 10 with a 30-second reset, and `--fault Goal=500` makes
  one endpoint fail. With `ICANOTES_KEYCHAIN_SERVICE` set, the vault lives
  beside the config (`vault.<service>/`), and `auth logout --forget` deletes
  its key item once nothing is left in it.

- A change to `KEY_EXPRESSION` in `src/login.rs` (the script that reads the
  key off the portal page) is checked against synthetic portal pages, decoy
  labels and buttons included, in headless Chromium:
  `PLAYWRIGHT_DIR=<dir whose node_modules has playwright> make
  check-key-expression`. CI has no browser, so this is not part of
  `make verify`.
- Test data is invented. Never commit a real response, record, export or
  ICANotes' documentation; `tests/no_personal_data.rs` and gitleaks check
  every change.
- Every change lands through a pull request with a review and green checks.
- Dates on the command line are `YYYY-MM-DD`. Every command takes `--json`,
  except `records export --ndjson`, whose stream is the output.
