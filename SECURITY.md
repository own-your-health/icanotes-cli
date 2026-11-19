# Security

## Reporting a vulnerability in icanotes

Report it privately through GitHub's private vulnerability reporting
(Security, Report a vulnerability) on
https://github.com/piekstra/icanotes-cli. Do not open a public issue, and do
not include a real record, patient id, key or secret in the report.

## A flaw in ICANotes' API

ICANotes' API terms ask that a breach of security or unauthorized use of a
credential be reported to ICANotes immediately. Report it to ICANotes, not
here.

## What icanotes guarantees

- The patient's API key and secret live only in icanotes' vault: files
  encrypted with a key that is one item in the OS credential store under the
  service `piekstra.icanotes`. They are never printed, logged, written to a
  file unencrypted, or taken from the command line.
- ICANotes takes the key and secret as query parameters, so icanotes never
  logs a request URL, strips URLs from transport errors, and redacts both
  values, raw and URL-encoded, from every error and from `api` output.
- The key is sent only to `api_base`: https on `icanotes.com` or a host under
  it, or `http` to this machine for a test server. No redirect is followed.
- icanotes' own files (the config with each profile's patient ids, the vault,
  `rate.json`, downloaded documents) are owner-only and written whole.
- icanotes reads; it never writes to ICANotes.
- The test suite and `make verify` never read the real Keychain
  (`ICANOTES_KEYCHAIN_SERVICE` points them at an empty service, and the vault
  at a directory beside their scratch config).
