// Evaluates src/login.rs's KEY_EXPRESSION in headless Chromium against
// synthetic FHIR API Access pages, decoys included (a label named
// lblUserSecret, a Generate button), and checks what it reads.
//
//   PLAYWRIGHT_DIR=<dir with node_modules/playwright> make check-key-expression
//
// Not part of `make verify`: it needs Playwright and a Chromium build.
import { createRequire } from 'module';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const dir = process.env.PLAYWRIGHT_DIR;
if (!dir) {
  console.error('set PLAYWRIGHT_DIR to a directory whose node_modules has playwright');
  process.exit(2);
}
const { chromium } = createRequire(path.join(dir, 'node_modules', '/'))('playwright');
const root = path.join(path.dirname(fileURLToPath(import.meta.url)), '..');
const src = fs.readFileSync(path.join(root, 'src', 'login.rs'), 'utf8');
const expr = src.match(/pub const KEY_EXPRESSION: &str = r#"([\s\S]*?)"#;/)[1];

const K = 'SYNkey0001abcd';
const S = 'SYN+secret/0001=';
const both = JSON.stringify({ key: K, secret: S });
const cases = [
  ['inputs named by their labels', both,
    `<label for="a">User Key</label><input id="a" value="${K}"><label for="b">User Secret</label><input id="b" value="${S}">`],
  ['a label element before each value', both,
    `<span id="lblUserKey">UserKey</span><input id="txtUserKey" value="${K}"><span id="lblUserSecret">UserSecret</span><input id="txtUserSecret" value="${S}">`],
  ['buttons named for the key', both,
    `<button id="btnGenerateUserKey">Generate</button><button id="btnCopyUserSecret">CopySecret</button><table><tr><th>User Key</th><td>${K}</td></tr><tr><th>User Secret</th><td>${S}</td></tr></table>`],
  ['a definition list', both, `<dl><dt>Key:</dt><dd>${K}</dd><dt>Secret:</dt><dd>${S}</dd></dl>`],
  ['text after the label', both, `<p>User Key: ${K}</p><p>User Secret: ${S}</p>`],
  ['nothing generated yet', null,
    `<h2>FHIR API Access</h2><span id="lblUserKey">UserKey</span><span id="lblUserSecret">UserSecret</span><button id="btnGenerate">Generate New Credentials</button>`],
  ['one value for both', null, `<p>User Key: ${K}</p><p>User Secret: ${K}</p>`],
  ['the portal turned off at sign-in', JSON.stringify({ stopped: 'portal-disabled' }),
    `<h1>Patient Portal Login</h1><div class="alert alert-warning">Use of the Patient Portal has been disabled. Please contact your provider with questions.</div><a class="btn btn-primary" href="/">Return to Login</a>`],
];

const browser = await chromium.launch({ headless: true });
const page = await browser.newPage();
let failed = 0;
for (const [name, want, html] of cases) {
  await page.setContent(`<!doctype html><html><body>${html}</body></html>`);
  const got = await page.evaluate(expr);
  const ok = got === want;
  if (!ok) failed++;
  console.log(`${ok ? 'ok    ' : 'FAILED'} ${name}${ok ? '' : `: got ${got}`}`);
}
await browser.close();
console.log(`${cases.length - failed} of ${cases.length} passed`);
process.exit(failed ? 1 : 0);
