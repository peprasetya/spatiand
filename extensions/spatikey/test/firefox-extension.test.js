// The Firefox build in Firefox: dist/spatikey-firefox installed as a temporary add-on in headless
// Firefox (over WebDriver BiDi), on a throwaway profile. No host is set up -- Firefox reads its
// native-messaging manifests from the user's own Mozilla folder, which a test must not touch --
// so this checks what runs without one: the manifest is taken, webauthn.js wraps the page's
// WebAuthn in the page's world, the key button shows in a login field, and pressing it (a
// person's click) reaches the background, which says SpatiKey's helper is not there.
//
//   node --test extensions/spatikey/test/*.test.js
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { spawn, spawnSync } = require('node:child_process');
const { serve, tempDir, ROOT, FIREFOXES } = require('./harness');

const FIREFOX = FIREFOXES.Firefox;

function version() {
  try {
    const plist = path.join(FIREFOX, '..', '..', 'Info.plist');
    const found = /<key>CFBundleShortVersionString<\/key>\s*<string>(\d+)/.exec(fs.readFileSync(plist, 'utf8'));
    return found ? Number(found[1]) : 0;
  } catch (e) {
    return 0;
  }
}
const skip = !fs.existsSync(FIREFOX) ? 'no Firefox' : version() < 128 ? 'Firefox older than 128' : false;

// A WebDriver BiDi session with a headless Firefox of its own.
async function firefox() {
  const profile = tempDir('firefox');
  fs.writeFileSync(path.join(profile, 'user.js'), [
    'user_pref("browser.shell.checkDefaultBrowser", false);',
    'user_pref("browser.aboutwelcome.enabled", false);',
    'user_pref("datareporting.policy.dataSubmissionEnabled", false);',
    'user_pref("app.update.disabledForTesting", true);',
    'user_pref("services.settings.server", "http://127.0.0.1:9/");'
  ].join('\n') + '\n');
  const child = spawn(FIREFOX, ['--headless', '--no-remote', '--profile', profile, '--remote-debugging-port', '0'], { stdio: ['ignore', 'ignore', 'pipe'] });
  const address = await new Promise((resolve, reject) => {
    let said = '';
    const timer = setTimeout(() => reject(new Error(`Firefox gave no BiDi address: ${said.slice(-500)}`)), 30000);
    child.stderr.on('data', (chunk) => {
      said += chunk;
      const found = /WebDriver BiDi listening on (ws:\/\/\S+)/.exec(said);
      if (found) {
        clearTimeout(timer);
        resolve(found[1]);
      }
    });
  });
  const socket = new WebSocket(`${address}/session`);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let next = 1;
  const calls = new Map();
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    const call = calls.get(message.id);
    if (!call) return;
    calls.delete(message.id);
    if (message.type === 'error') call.reject(new Error(`${call.method}: ${message.error}: ${message.message}`));
    else call.resolve(message.result);
  };
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = next++;
    calls.set(id, { resolve, reject, method });
    socket.send(JSON.stringify({ id, method, params }));
  });
  await send('session.new', { capabilities: {} });
  return {
    send,
    async stop() {
      try {
        await Promise.race([send('session.end'), new Promise((resolve) => setTimeout(resolve, 2000))]);
      } catch (e) {}
      socket.close();
      child.kill('SIGTERM');
      // Firefox may have started itself again behind the process that was started: end every
      // process of this profile.
      const name = path.basename(profile);
      for (let i = 0; i < 100 && spawnSync('pgrep', ['-f', name]).status === 0; i++) {
        spawnSync('pkill', [i < 50 ? '-TERM' : '-KILL', '-f', name]);
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      fs.rmSync(profile, { recursive: true, force: true });
    }
  };
}

async function until(fn, what, ms = 10000) {
  const end = Date.now() + ms;
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() > end) throw new Error(`waited ${ms} ms for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

test('the Firefox build in Firefox: taken, in the page, and asking its background', { skip, timeout: 120000 }, async () => {
  // Built into a folder of its own: other tests build at the same time.
  const out = tempDir('build');
  const built = spawnSync('bash', [path.join(ROOT, 'build.sh'), out], { encoding: 'utf8' });
  assert.equal(built.status, 0, built.stderr);
  const server = await serve();
  const browser = await firefox();
  try {
    const { send } = browser;
    const { extension } = await send('webExtension.install', { extensionData: { type: 'path', path: path.join(out, 'spatikey-firefox') } });
    assert.equal(extension, 'spatikey@prasetya.id');
    const { contexts } = await send('browsingContext.getTree', {});
    const context = contexts[0].context;
    const evaluate = async (expression) => {
      const out = await send('script.evaluate', { expression, target: { context }, awaitPromise: true, resultOwnership: 'none' });
      if (out.type !== 'success') throw new Error(`${expression.slice(0, 60)}: ${JSON.stringify(out.exceptionDetails)}`);
      return out.result.value;
    };
    const click = async (selector) => {
      const at = JSON.parse(await evaluate(`(() => { const r = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return JSON.stringify({ x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) }); })()`));
      await send('input.performActions', { context, actions: [{ type: 'pointer', id: 'mouse', parameters: { pointerType: 'mouse' }, actions: [
        { type: 'pointerMove', x: at.x, y: at.y }, { type: 'pointerDown', button: 0 }, { type: 'pointerUp', button: 0 }
      ] }] });
    };
    await send('browsingContext.navigate', { context, url: server.url('/pages/extension.html'), wait: 'complete' });
    await until(() => evaluate(`!/\\[native code\\]/.test(Function.prototype.toString.call(CredentialsContainer.prototype.get))`), 'webauthn.js wrapping Firefox\'s own');
    assert.equal(await evaluate(`typeof SpatiKeyForms + typeof chrome`), 'undefinedundefined', 'the content script\'s world is not the page\'s');
    await click('#username');
    // Headless Firefox moves the focus on a click but sends no focus events (not even to the
    // page's own listeners): the one a window would get is sent by hand. The click on the key
    // button below is a person's, as the extension wants it.
    await evaluate(`document.querySelector('#username').dispatchEvent(new FocusEvent('focusin', { bubbles: true, composed: true }))`);
    await until(() => evaluate(`(() => { const h = document.querySelector('spatikey-key'); return !!h && h.getBoundingClientRect().width > 0; })()`), 'the key button in the field');
    await click('spatikey-key');
    await until(() => evaluate(`!!document.querySelector('spatikey-note')`), 'the note that SpatiKey\'s helper is not there');
  } finally {
    await browser.stop();
    await server.close();
    fs.rmSync(out, { recursive: true, force: true });
  }
});
