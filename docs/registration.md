# Setting up icanotes

icanotes is for a patient of a practice that keeps its records in ICANotes
and gives its patients the ICANotes patient portal
(https://patientonlineportal.com). There is no app to register with
ICANotes. The patient API takes a key and secret that the patient generates
on the portal, and icanotes stores that pair.

Setup has three parts:

1. The practice gives you a portal account.
2. You generate a key and secret on the portal.
3. icanotes stores the pair.

## 1. A portal account, from the practice

- The portal has no self sign-up. Its sign-in page offers only "Forgot
  Password?" and a "Portal Admin Login" for practice staff, so the practice
  creates your account and sends the invitation. Ask the front desk.
- A forgotten password resets through "Forgot Password?" on the sign-in
  page.
- The practice can turn the portal off for a login. Signing in then stays on
  the sign-in page and shows:

  > Use of the Patient Portal has been disabled. Please contact your
  > provider with questions.

  It shows a Return to Login button and nothing else. This is not a wrong
  password: the portal is off for this account, and nothing you do on the
  page turns it back on. Ask the practice to turn the patient portal back
  on. icanotes 0.1.1 and later ends `auth login` here with exit 3; 0.1.0
  waits until `--timeout` runs out.
- The key comes from the portal. ICANotes' API documentation: "You must
  obtain the userKey and userSecret by going to the patient portal of
  ICANotes." Whether a practice can hide the FHIR API Access page while
  leaving the rest of the portal on is not confirmed. If the page is missing
  once you are signed in, ask the practice to turn on patient API access.

The page names below (My History, FHIR API Access, Generate New Credentials)
come from ICANotes' documentation. They have not yet been checked against a
signed-in account (`docs/api.md`, Signed-in evidence).

## 2. Install icanotes and add a profile

Install it as the [README](../README.md#install) says. A profile is one
portal login. Name it for the person, as in `me`:

```sh
icanotes profiles add me
```

Every command that uses a key needs `--profile <NAME>` or
`ICANOTES_PROFILE`. There is no default.

## 3. Generate the key and store it

```sh
icanotes auth login --browser --profile me --label me
```

1. icanotes opens Chrome at the portal's sign-in, in the profile's own
   browser profile
   (`~/Library/Application Support/piekstra.icanotes/browser-profile-me` on
   macOS).
2. Sign in.
3. Open My History, then FHIR API Access, and choose Generate New
   Credentials.
4. icanotes reads the key and secret off that page. It checks them with one
   `PatientID` request, which also gives the patient id, stores them, and
   closes the window. Neither is printed.

`--timeout <MIN>` (default 15) bounds the wait. `--label` names the patient
for `--patient` later.

These steps need a person:
- the portal password, typed or filled by a password manager;
- any code, terms or consent prompt the portal shows;
- on macOS, the Keychain prompt the first time a newly installed icanotes
  stores or reads a key. Choose Always Allow. Each new build asks once.

`icanotes auth logout --profile me --forget` later removes the stored pair
and, once no profile holds one, icanotes' key in the Keychain.

### A key you copied yourself

The page shows the secret once. Copy both values, then store them from
stdin, as `<key>:<secret>` or as two lines:

```sh
pbpaste | icanotes auth set-credential --profile me --stdin [--label me]
```

`--from-env <VAR>` reads them from an environment variable instead. With
neither flag, icanotes asks for them at a prompt that does not echo. Never
put them in a command's arguments. icanotes checks the pair with ICANotes
before storing it; `--no-verify --patient-id <ID>` stores it unchecked.

## 4. Check it

```sh
icanotes auth status --profile me --check   # stored, and accepted by ICANotes
icanotes info                               # setup.configured, keys per profile
```

Then export, or let medrec run the export:

```sh
icanotes records export --profile me --ndjson > me.ndjson
medrec --profile <FOLDER> import --from icanotes --from-profile me --patient me
```

## How long it lasts

- A pair does not expire. ICANotes' documentation: "userTokens do not get
  deleted unless you delete them from the Patient Portal page yourself."
- Generating another pair leaves earlier ones valid.
- To revoke a pair, delete it on the FHIR API Access page.
  `icanotes auth logout` and `icanotes profiles remove` forget only
  icanotes' copy.
- The portal sign-in is needed only to generate a pair. Exports use the
  pair alone and never sign in to the portal.
- A pair deleted on the portal ends in exit 3 with "ICANotes refused the
  API key (401)". Generate a new one.

## Where it is kept

| What | Where |
|---|---|
| The key and secret | icanotes' vault, `~/.config/icanotes/vault/`, encrypted with a key held in the macOS Keychain (service `piekstra.icanotes`) |
| Profiles, labels, patient ids, settings | `~/.config/icanotes/config.json`, owner-only, no secret |
| The portal's cookies | the profile's browser profile (above); `profiles remove` deletes it |
| ICANotes' request window | `~/.config/icanotes/rate.json`, shared by every run |

## Terms

- ICANotes' API documentation asks developers not to share copies of it.
  This repository restates in its own words what icanotes depends on, and
  quotes a sentence at most.
- The key is the patient's own: it reads that patient's chart and nothing
  else. Share it the way you would share the chart.
- The portal's own terms are on the portal; read them there.

## When something goes wrong

| What you see | What it means | Do |
|---|---|---|
| "Use of the Patient Portal has been disabled. Please contact your provider with questions." | the practice turned the portal off for this login | ask the practice to turn it back on |
| no FHIR API Access under My History | not confirmed; likely patient API access is off | ask the practice to turn on patient API access |
| exit 3, "holds no ICANotes API key" | nothing stored for the profile | step 3 |
| exit 3, "ICANotes refused the API key (401)" | the pair was deleted or replaced on the portal | generate a new pair (step 3) |
| exit 3, `auth login` timed out | the window closed, or the key never appeared | run `auth login` again and finish the steps in the window |
| a command waits and prints nothing | a macOS Keychain prompt is open | approve it on the Mac |
