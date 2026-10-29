// The Chrome extension, whole: build.sh's dist/spatikey-chrome loaded into headless Chrome (through
// the DevTools protocol, as "Load unpacked" would), a stand-in native host (fake-host.js) named in
// the throwaway profile's own NativeMessagingHosts folder, and a virtual authenticator standing in
// for the browser's own WebAuthn. A page makes and uses passkeys, fills a login with the key
// button and the chooser, and saves a sign-up, with a person's clicks and keys.
//
//   node --test extensions/spatikey/test/*.test.js
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { serve, chromeThere, CHROME, ROOT, tempDir } = require('./harness');
const { launch, openTab, sleep, until } = require('./cdp');

const skip = chromeThere() ? false : `no Chrome at ${CHROME}`;
// What manifest.chrome.json's key makes of the extension's id; README.md and
// tools/spatikey-browsers.sh name it.
const ID = 'ndebkmflcohflpgdclfbnjjeogbigpib';
const b64 = (text) => Buffer.from(text).toString('base64url');
const sha = (text) => crypto.createHash('sha256').update(text).digest('base64url');
const CHALLENGE = b64(Buffer.from([0xfb, 0xff, 0xbf, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]));

// A node inside the closed shadow root of `host` (a tag name) that `match` takes, as DevTools
// sees it; the page cannot.
async function inShadow(tab, host, match) {
  const { root } = await tab.send('DOM.getDocument', { depth: -1, pierce: true });
  let found = null;
  (function walk(node, inside) {
    if (found) return;
    const here = inside || node.nodeName === host.toUpperCase();
    if (here && match(node)) {
      found = node;
      return;
    }
    for (const child of node.children || []) walk(child, here);
    for (const shadow of node.shadowRoots || []) walk(shadow, here);
  })(root, false);
  return found;
}

const labelled = (label) => (node) => node.nodeName === 'BUTTON' && (node.children || []).some((c) => c.nodeType === 3 && c.nodeValue === label);
const saying = (text) => (node) => node.nodeType === 3 && node.nodeValue === text;

async function clickNode(tab, node) {
  const { model } = await tab.send('DOM.getBoxModel', { nodeId: node.nodeId });
  const q = model.content;
  await tab.click((q[0] + q[4]) / 2, (q[1] + q[5]) / 2);
}

async function pressKeyButton(tab) {
  const at = await until(() => tab.evaluate(`(() => {
    const host = document.querySelector('spatikey-key');
    if (!host) return null;
    const r = host.getBoundingClientRect();
    return r.width ? { x: r.x + r.width / 2, y: r.y + r.height / 2 } : null;
  })()`), 'the key button in the field');
  await tab.click(at.x, at.y);
}

async function chooseSecond(tab) {
  await until(() => tab.evaluate(`!!document.querySelector('spatikey-choose')`), 'the chooser');
  await sleep(100);
  await tab.key('ArrowDown');
  await tab.key('Enter');
}

test('the extension in Chrome: passkeys, filling and saving through SpatiKey', { skip, timeout: 180000 }, async () => {
  // Built into a folder of its own: other tests build at the same time.
  const out = tempDir('build');
  const built = spawnSync('bash', [path.join(ROOT, 'build.sh'), out], { encoding: 'utf8' });
  assert.equal(built.status, 0, built.stderr);
  const work = tempDir('host');
  const profile = tempDir('extension');
  const log = path.join(work, 'requests.jsonl');
  const host = path.join(work, 'spatikey-host');
  fs.writeFileSync(host, `#!/bin/sh\nexec "${process.execPath}" "${path.join(__dirname, 'fake-host.js')}" "$@"\n`, { mode: 0o755 });
  fs.mkdirSync(path.join(profile, 'NativeMessagingHosts'));
  fs.writeFileSync(path.join(profile, 'NativeMessagingHosts', 'id.prasetya.spatikey.json'), JSON.stringify({
    name: 'id.prasetya.spatikey', description: 'a stand-in for spatikey-host', path: host, type: 'stdio', allowed_origins: [`chrome-extension://${ID}/`]
  }));
  const server = await serve();
  const browser = await launch(['--enable-unsafe-extension-debugging'], { profile, env: { SPATIKEY_FAKE_LOG: log } });
  const requests = () => (fs.existsSync(log) ? fs.readFileSync(log, 'utf8').split('\n').filter(Boolean).map((line) => JSON.parse(line)) : []);
  try {
    const { id } = await browser.cdp.send('Extensions.loadUnpacked', { path: path.join(out, 'spatikey-chrome') });
    assert.equal(id, ID, 'the manifest\'s key gives the extension the id the host manifests allow');
    const tab = await openTab(browser.cdp);
    await tab.send('Page.enable');
    await tab.send('Runtime.enable');
    await tab.send('DOM.enable');
    await tab.send('Emulation.setFocusEmulationEnabled', { enabled: true });
    await tab.send('WebAuthn.enable', { enableUI: false });
    const { authenticatorId } = await tab.send('WebAuthn.addVirtualAuthenticator', { options: {
      protocol: 'ctap2', ctap2Version: 'ctap2_1', transport: 'internal', hasResidentKey: true, hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true
    } });
    await tab.navigate(server.url('/pages/extension.html'));
    const origin = `http://localhost:${server.port}`;
    await until(() => tab.evaluate(`!/\\[native code\\]/.test(Function.prototype.toString.call(CredentialsContainer.prototype.get))`), 'webauthn.js wrapping the browser\'s own');

    // A conditional get waits for the key button, then signs in with SpatiKey's passkey; the
    // browser's own wait is stopped.
    assert.equal(await tab.evaluate('waitConditional()'), 'waiting');
    await sleep(200);
    assert.equal(requests().length, 0, 'nothing asked before the person asks');
    await tab.clickOn('#username');
    await pressKeyButton(tab);
    const conditional = await until(() => tab.evaluate('window.conditional'), 'the conditional get\'s answer');
    assert.equal(conditional.id, b64('spatikey-paulus'));
    assert.equal(conditional.kind, 'assertion');
    assert.equal(conditional.ownGetters, true, 'SpatiKey\'s, as the browser\'s own classes');
    assert.equal(conditional.data, `{"type":"webauthn.get","challenge":"${CHALLENGE}","origin":"${origin}","crossOrigin":false}`);
    const asked = requests()[0];
    assert.deepEqual(asked, { id: asked.id, who: { app: 'Chrome' }, op: 'passkey_get', origin, rp_id: 'localhost', client_data_hash: sha(conditional.data), allow: [], conditional: true });

    // A passkey made in SpatiKey.
    const made = await tab.evaluate(`make('paulus')`);
    const data = `{"type":"webauthn.create","challenge":"${CHALLENGE}","origin":"${origin}","crossOrigin":false}`;
    assert.deepEqual(made, {
      pkc: true, ownGetters: true, id: b64('spatikey-made'), type: 'public-key', data, kind: 'attestation', alg: -7, transports: ['hybrid', 'internal'],
      json: {
        id: b64('spatikey-made'), rawId: b64('spatikey-made'), type: 'public-key', authenticatorAttachment: 'platform', clientExtensionResults: {},
        response: { clientDataJSON: b64(data), authenticatorData: b64('auth-data'), transports: ['hybrid', 'internal'], publicKey: b64('spki'), publicKeyAlgorithm: -7, attestationObject: b64('attestation') }
      }
    });
    const create = requests().find((r) => r.op === 'passkey_create');
    assert.deepEqual(create, {
      id: create.id, who: { app: 'Chrome' }, op: 'passkey_create', origin, rp_id: 'localhost', rp_name: 'Example', user_id: b64('user-paulus'),
      user_name: 'paulus', user_display: 'PAULUS', client_data_hash: sha(data), exclude: [], algs: [-7]
    });

    // SpatiKey not set up: the browser's own makes it.
    const native = await tab.evaluate(`make('use-the-browser')`);
    assert.equal(native.pkc, true);
    assert.equal(native.ownGetters, false, 'the browser\'s own object');
    assert.equal(JSON.parse(native.data).origin, origin);
    const { credentials } = await tab.send('WebAuthn.getCredentials', { authenticatorId });
    assert.equal(credentials.length, 1);

    // An account SpatiKey already has a passkey for.
    assert.deepEqual(await tab.evaluate(`make('taken')`), { error: 'InvalidStateError', message: 'The user attempted to register an authenticator that contains one of the credentials already registered with the relying party.' });

    // A sign-in SpatiKey has nothing for goes to the browser's own.
    const viaBrowser = await tab.evaluate(`signIn([${JSON.stringify(native.id)}])`);
    assert.equal(viaBrowser.id, native.id);
    assert.equal(viaBrowser.kind, 'assertion');
    assert.equal(viaBrowser.ownGetters, false);

    // Two accounts: the person chooses the second, and the page gets only that one.
    const pending = tab.evaluate(`signIn([])`);
    await chooseSecond(tab);
    const chosen = await pending;
    assert.equal(chosen.id, b64('spatikey-work'));
    assert.equal(chosen.signature, b64('sig-2'));
    assert.equal(chosen.userHandle, b64('uh-2'));
    const lastGet = requests().filter((r) => r.op === 'passkey_get').pop();
    assert.equal(lastGet.client_data_hash, sha(chosen.data));

    // The key button in the password field fills the login chosen, in both fields.
    await tab.clickOn('#password');
    await pressKeyButton(tab);
    await chooseSecond(tab);
    await until(() => tab.evaluate(`document.querySelector('#password').value === 'second-pw'`), 'the login filled');
    assert.equal(await tab.evaluate(`document.querySelector('#username').value`), 'work@example.com');
    const fill = requests().find((r) => r.op === 'fill');
    assert.deepEqual(fill, { id: fill.id, who: { app: 'Chrome' }, op: 'fill', origin });

    // A sign-up the person types and sends is offered for saving, and saved on yes.
    await tab.clickOn('#new-user');
    await tab.type('new@example.com');
    await tab.clickOn('#new-password');
    await tab.type('n3w-secret!');
    await tab.clickOn('#create');
    assert.equal(await tab.evaluate('window.signedUp'), true);
    await until(() => inShadow(tab, 'spatikey-save', saying('Save the new password for new@example.com in SpatiKey?')), 'the save bar');
    // (Each look at the document renumbers its nodes: the button is found last, then pressed.)
    await clickNode(tab, await inShadow(tab, 'spatikey-save', labelled('Save')));
    await until(() => inShadow(tab, 'spatikey-save', saying('Saved in SpatiKey.')), 'the bar saying it is saved');
    const saved = requests().find((r) => r.op === 'save');
    assert.deepEqual(saved, { id: saved.id, who: { app: 'Chrome' }, op: 'save', origin, title: '', username: 'new@example.com', password: 'n3w-secret!' });
    assert.deepEqual(requests().map((r) => r.op), ['passkey_get', 'passkey_create', 'passkey_create', 'passkey_create', 'passkey_get', 'passkey_get', 'fill', 'save']);
  } finally {
    await browser.stop();
    await server.close();
    fs.rmSync(work, { recursive: true, force: true });
    fs.rmSync(out, { recursive: true, force: true });
  }
});
