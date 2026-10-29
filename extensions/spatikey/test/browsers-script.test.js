// tools/spatikey-browsers.sh, run for real but only ever with HOME in a throwaway folder: the
// native-messaging manifests it writes for each browser, on the Mac and (with a stand-in uname
// saying so) on Linux, --print, --all, --remove, and the note about Flatpak browsers.
//
//   node --test extensions/spatikey/test/*.test.js
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { REPO, tempDir } = require('./harness');

const SCRIPT = path.join(REPO, 'tools', 'spatikey-browsers.sh');
const CHROME_ORIGIN = 'chrome-extension://ndebkmflcohflpgdclfbnjjeogbigpib/';

// A home of its own, and stand-ins for programs (uname, flatpak) put first on PATH.
function home(t, stand = {}) {
  const dir = tempDir('home');
  const bin = path.join(dir, '.bin');
  fs.mkdirSync(bin);
  for (const [name, body] of Object.entries(stand)) fs.writeFileSync(path.join(bin, name), `#!/bin/sh\n${body}\n`, { mode: 0o755 });
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const run = (...args) => {
    const env = Object.assign({}, process.env, { HOME: dir, PATH: `${bin}:${process.env.PATH}` });
    delete env.XDG_CONFIG_HOME;
    const out = spawnSync('bash', [SCRIPT, ...args], { env, encoding: 'utf8', cwd: dir });
    assert.equal(out.status, 0, out.stderr);
    return out;
  };
  return { dir, run, mk: (p) => fs.mkdirSync(path.join(dir, p), { recursive: true }), read: (p) => JSON.parse(fs.readFileSync(path.join(dir, p), 'utf8')) };
}

function files(dir) {
  const out = [];
  (function walk(d) {
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      if (e.name === '.bin') continue;
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else out.push(path.relative(dir, p));
    }
  })(dir);
  return out.sort();
}

const MAC = 'Library/Application Support';

test('Mac: a manifest for each browser that is here, naming the host', (t) => {
  const h = home(t, { uname: 'echo Darwin' });
  assert.match(h.run('/opt/spatikey-host').stdout, /no browser here to set up/);
  assert.deepEqual(files(h.dir), []);
  h.mk(`${MAC}/Google/Chrome`);
  h.mk(`${MAC}/Mozilla`);
  const out = h.run('/opt/spatikey-host');
  assert.match(out.stdout, /Chrome: wrote/);
  assert.match(out.stdout, /Firefox: wrote/);
  assert.deepEqual(files(h.dir), [`${MAC}/Google/Chrome/NativeMessagingHosts/id.prasetya.spatikey.json`, `${MAC}/Mozilla/NativeMessagingHosts/id.prasetya.spatikey.json`]);
  assert.deepEqual(h.read(`${MAC}/Google/Chrome/NativeMessagingHosts/id.prasetya.spatikey.json`), {
    name: 'id.prasetya.spatikey', description: 'SpatiKey, Spatiand\'s passwords and passkeys', path: '/opt/spatikey-host', type: 'stdio', allowed_origins: [CHROME_ORIGIN]
  });
  assert.deepEqual(h.read(`${MAC}/Mozilla/NativeMessagingHosts/id.prasetya.spatikey.json`), {
    name: 'id.prasetya.spatikey', description: 'SpatiKey, Spatiand\'s passwords and passkeys', path: '/opt/spatikey-host', type: 'stdio', allowed_extensions: ['spatikey@prasetya.id']
  });
});

test('Mac: the default host is Spatiand.app\'s; --print writes nothing; --all and --remove', (t) => {
  const h = home(t, { uname: 'echo Darwin' });
  h.mk(`${MAC}/BraveSoftware/Brave-Browser`);
  const shown = h.run('--print');
  assert.match(shown.stdout, /Brave: would write .*Brave-Browser\/NativeMessagingHosts\/id\.prasetya\.spatikey\.json/);
  assert.ok(shown.stdout.includes(`"path": "${h.dir}/Applications/Spatiand.app/Contents/MacOS/spatikey-host"`));
  assert.match(shown.stdout, /Edge: not here/);
  assert.match(shown.stderr, /is not there/);
  assert.deepEqual(files(h.dir), []);
  h.run('--all', '/opt/host');
  assert.equal(files(h.dir).length, 7);
  for (const f of files(h.dir)) assert.equal(h.read(f).path, '/opt/host');
  assert.match(h.run('--remove', '--print').stdout, /Chrome Canary: would remove/);
  assert.equal(files(h.dir).length, 7);
  h.run('--remove');
  assert.deepEqual(files(h.dir), []);
});

test('a relative host becomes absolute, and odd paths stay valid JSON', (t) => {
  const h = home(t, { uname: 'echo Darwin' });
  h.mk(`${MAC}/Google/Chrome`);
  h.run('bin/spatikey-host');
  // (Taken from where it runs: the folder as the shell has it, links resolved.)
  assert.equal(h.read(`${MAC}/Google/Chrome/NativeMessagingHosts/id.prasetya.spatikey.json`).path, `${fs.realpathSync(h.dir)}/bin/spatikey-host`);
  h.run('/opt/a "quoted" \\ path/spatikey-host');
  assert.equal(h.read(`${MAC}/Google/Chrome/NativeMessagingHosts/id.prasetya.spatikey.json`).path, '/opt/a "quoted" \\ path/spatikey-host');
});

test('Linux: the browsers\' own folders, and the host beside spatiand', (t) => {
  const h = home(t, { uname: 'echo Linux' });
  h.mk('.config/google-chrome');
  h.mk('.config/chromium');
  h.mk('.mozilla');
  h.run();
  assert.deepEqual(files(h.dir), [
    '.config/chromium/NativeMessagingHosts/id.prasetya.spatikey.json',
    '.config/google-chrome/NativeMessagingHosts/id.prasetya.spatikey.json',
    '.mozilla/native-messaging-hosts/id.prasetya.spatikey.json'
  ]);
  assert.equal(h.read('.config/google-chrome/NativeMessagingHosts/id.prasetya.spatikey.json').path, `${h.dir}/spatiand/target/release/spatikey-host`);
  assert.deepEqual(h.read('.mozilla/native-messaging-hosts/id.prasetya.spatikey.json').allowed_extensions, ['spatikey@prasetya.id']);
  // With spatiand on PATH, beside that one.
  const k = home(t, { uname: 'echo Linux', spatiand: 'true' });
  k.mk('.config/microsoft-edge');
  k.run();
  assert.equal(k.read('.config/microsoft-edge/NativeMessagingHosts/id.prasetya.spatikey.json').path, `${k.dir}/.bin/spatikey-host`);
});

test('Linux: Flatpak browsers are named, and left alone', (t) => {
  const h = home(t, { uname: 'echo Linux', flatpak: 'echo org.mozilla.firefox; echo com.valvesoftware.Steam; echo com.google.Chrome' });
  const out = h.run('--all', '/home/deck/spatiand/target/release/spatikey-host');
  assert.match(out.stderr, /Flatpaks, which this does not set up/);
  assert.match(out.stderr, /org\.mozilla\.firefox/);
  assert.match(out.stderr, /com\.google\.Chrome/);
  assert.doesNotMatch(out.stderr, /Steam/);
  assert.ok(!files(h.dir).some((f) => f.startsWith('.var')), 'nothing written into a sandbox');
});
