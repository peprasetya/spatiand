// background.js in node, against stand-ins for the browser's extension API and for SpatiKey's
// native host: the origins it accepts, clientDataJSON and its hash, the service's requests it
// sends, the answers it gives the page, choosing between accounts, filling and saving.
//
//   node --test extensions/spatikey/test/*.test.js
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const SOURCE = fs.readFileSync(path.join(__dirname, '..', 'src', 'background.js'), 'utf8');
const ID = 'ndebkmflcohflpgdclfbnjjeogbigpib';

// The background, with `host` standing in for SpatiKey: it is given each request and returns
// {ok} or {err}, null for no answer, or 'gone' to end the connection as a missing host does.
function background({ host = () => ({ ok: {} }), choose = () => ({ choice: 0 }), userAgent = 'Mozilla/5.0 Chrome/156.0.0.0', brands = ['Google Chrome', 'Chromium'] } = {}) {
  const asked = [];
  const toTabs = [];
  let onMessage = null, onRemoved = null, connections = 0;
  const chrome = {
    runtime: {
      id: ID,
      lastError: undefined,
      onMessage: { addListener: (fn) => { onMessage = fn; } },
      connectNative(name) {
        assert.equal(name, 'id.prasetya.spatikey');
        connections++;
        const heard = [], ended = [];
        const port = {
          onMessage: { addListener: (fn) => heard.push(fn) },
          onDisconnect: { addListener: (fn) => ended.push(fn) },
          disconnect() {},
          postMessage(message) {
            asked.push(JSON.parse(JSON.stringify(message)));
            const answer = host(message);
            if (answer === 'gone') {
              setImmediate(() => {
                chrome.runtime.lastError = { message: 'Specified native messaging host not found.' };
                ended.forEach((fn) => fn(port));
                chrome.runtime.lastError = undefined;
              });
            } else if (answer) {
              setImmediate(() => heard.forEach((fn) => fn(Object.assign({ id: message.id }, answer))));
            }
          }
        };
        return port;
      }
    },
    tabs: {
      sendMessage(tab, message, options) {
        const copy = { tab, message: JSON.parse(JSON.stringify(message)), options: JSON.parse(JSON.stringify(options)) };
        toTabs.push(copy);
        return Promise.resolve(copy.message.k === 'choose' ? choose(copy.message, copy.options) : undefined);
      },
      onRemoved: { addListener: (fn) => { onRemoved = fn; } }
    }
  };
  const context = {
    module: { exports: {} },
    chrome,
    navigator: { userAgent, userAgentData: { brands: brands.map((brand) => ({ brand, version: '156' })) } },
    crypto: globalThis.crypto, TextEncoder, URL, btoa, console,
    // The background's waits are minutes long: they must not keep the tests running.
    setTimeout: (fn, ms) => { const timer = setTimeout(fn, ms); timer.unref(); return timer; },
    clearTimeout
  };
  vm.createContext(context);
  vm.runInContext(SOURCE, context, { filename: 'background.js' });
  return {
    asked, toTabs, chrome,
    helpers: context.module.exports,
    connections: () => connections,
    // What a content script would get back for `message`, sent from `sender`: copied, as the
    // browser's messaging copies it.
    send(message, sender) {
      return new Promise((resolve) => {
        const respond = (answer) => resolve(answer === undefined ? undefined : JSON.parse(JSON.stringify(answer)));
        const open = onMessage(message, Object.assign({ id: ID }, sender), respond);
        if (open !== true) resolve(undefined);
      });
    },
    closeTab: (tab) => onRemoved(tab)
  };
}

const TOP = { tab: { id: 7 }, frameId: 0, origin: 'https://example.com', url: 'https://example.com/login', documentId: 'doc-a' };
const b64url = (text) => Buffer.from(text).toString('base64url');
const sha = (text) => crypto.createHash('sha256').update(text).digest('base64url');

const CREATE = { t: 'create', id: 1, challenge: 'AAEC', rp: { name: 'Example' }, user: { id: 'AQID', name: 'paulus', displayName: 'Paulus' }, algs: [-7, -257], exclude: ['CQk', 'bad+id'], timeout: 60000 };
const GET = { t: 'get', id: 2, challenge: 'AAEC', allow: [], conditional: false, timeout: 60000 };
const CREATED = { credential_id: 'Y3JlZA', authenticator_data: 'YXV0aA', attestation_object: 'YXR0', public_key: 'c3BraQ', alg: -7, transports: ['hybrid', 'internal'] };
const choice = (name, n) => ({ user_name: name, user_display: name.toUpperCase(), credential_id: 'id' + n, authenticator_data: 'ad' + n, signature: 'sig' + n, user_handle: 'uh' + n });

test('clientDataJSON is what browsers write, and its hash what SpatiKey is given', async () => {
  const { clientDataJSON, sha256, b64url: encode } = background().helpers;
  assert.equal(clientDataJSON('webauthn.create', 'AAEC', 'https://example.com', false, null),
    '{"type":"webauthn.create","challenge":"AAEC","origin":"https://example.com","crossOrigin":false}');
  assert.equal(clientDataJSON('webauthn.get', 'x', 'https://a.example', true, 'https://b.example'),
    '{"type":"webauthn.get","challenge":"x","origin":"https://a.example","crossOrigin":true,"topOrigin":"https://b.example"}');
  // A challenge cannot break out of its string.
  assert.equal(JSON.parse(clientDataJSON('webauthn.get', 'a","origin":"https://evil', 'https://a.example', false)).origin, 'https://a.example');
  assert.equal(await sha256('hello'), sha('hello'));
  assert.equal(encode(new Uint8Array([0xfb, 0xff, 0xbf, 1])), Buffer.from([0xfb, 0xff, 0xbf, 1]).toString('base64url'));
});

test('origins: the browser\'s word, and only secure ones', () => {
  const { originOf, isSecure, hostOf } = background().helpers;
  assert.equal(originOf({ origin: 'https://a.example', url: 'https://b.example/' }), 'https://a.example');
  assert.equal(originOf({ url: 'https://b.example:8443/x?y' }), 'https://b.example:8443');
  assert.equal(originOf({ origin: 'null', url: 'about:blank' }), null);
  assert.equal(originOf({}), null);
  for (const good of ['https://example.com', 'http://localhost:8080', 'http://app.localhost', 'http://127.0.0.1:3000', 'http://127.9.9.9', 'http://[::1]:5000']) {
    assert.equal(isSecure(good), true, good);
  }
  for (const bad of ['http://example.com', 'http://192.168.1.2', 'file:///etc/passwd', 'chrome-extension://abc', 'ftp://x', 'nonsense', 'http://localhost.example.com']) {
    assert.equal(isSecure(bad), false, bad);
  }
  assert.equal(hostOf('http://[::1]:5000'), '::1');
  assert.equal(hostOf('https://Login.Example.COM'), 'login.example.com');
});

test('relying party ids: the host or a parent of it', () => {
  const { rpIdOk } = background().helpers;
  assert.equal(rpIdOk('example.com', 'example.com'), true);
  assert.equal(rpIdOk('example.com', 'login.example.com'), true);
  assert.equal(rpIdOk('Example.COM', 'login.example.com'), true);
  assert.equal(rpIdOk('other.com', 'example.com'), false);
  assert.equal(rpIdOk('ample.com', 'example.com'), false);
  assert.equal(rpIdOk('login.example.com', 'example.com'), false);
  assert.equal(rpIdOk('com', 'example.com'), false);
  assert.equal(rpIdOk('', 'example.com'), false);
  assert.equal(rpIdOk('192.168.1.2', '192.168.1.2'), true);
  assert.equal(rpIdOk('168.1.2', '192.168.1.2'), false);
  assert.equal(rpIdOk('localhost', 'localhost'), true);
  assert.equal(rpIdOk('localhost', 'app.localhost'), true);
  assert.equal(rpIdOk('localhost', 'example.com'), false);
});

test('a passkey is made with the browser\'s origin and the hash of clientDataJSON', async () => {
  const b = background({ host: () => ({ ok: CREATED }) });
  const answer = await b.send({ k: 'webauthn', doc: 'd1', request: Object.assign({}, CREATE, { origin: 'https://evil.example' }) }, TOP);
  const data = '{"type":"webauthn.create","challenge":"AAEC","origin":"https://example.com","crossOrigin":false}';
  assert.equal(b.asked.length, 1);
  const sent = b.asked[0];
  assert.equal(typeof sent.id, 'number');
  assert.deepEqual(sent.who, { app: 'Chrome' });
  assert.deepEqual(Object.assign({}, sent, { id: 0, who: null }), {
    id: 0, who: null, op: 'passkey_create', origin: 'https://example.com', rp_id: 'example.com', rp_name: 'Example',
    user_id: 'AQID', user_name: 'paulus', user_display: 'Paulus', client_data_hash: sha(data), exclude: ['CQk'], algs: [-7, -257]
  });
  assert.deepEqual(answer, { ok: Object.assign({}, CREATED, { client_data_json: b64url(data) }) });
});

test('the relying party id: a parent domain is taken, another site is a SecurityError, nothing is asked', async () => {
  const b = background({ host: () => ({ ok: CREATED }) });
  const login = Object.assign({}, TOP, { origin: 'https://login.example.com', url: 'https://login.example.com/' });
  await b.send({ k: 'webauthn', request: Object.assign({}, CREATE, { rp: { id: 'example.com', name: 'X' } }) }, login);
  assert.equal(b.asked[0].rp_id, 'example.com');
  assert.equal(b.asked[0].origin, 'https://login.example.com');
  assert.deepEqual((await b.send({ k: 'webauthn', request: Object.assign({}, CREATE, { rp: { id: 'other.com', name: 'X' } }) }, login)).err.code, 'security');
  assert.deepEqual((await b.send({ k: 'webauthn', request: Object.assign({}, GET, { rpId: 'other.com' }) }, login)).err.code, 'security');
  assert.deepEqual((await b.send({ k: 'webauthn', request: Object.assign({}, GET, { challenge: 'not base64url!' }) }, login)).err.code, 'bad_request');
  assert.equal(b.asked.length, 1);
});

test('pages that are not secure, and frames of another site, are left to the browser', async () => {
  const b = background({ host: () => ({ ok: CREATED }) });
  const insecure = Object.assign({}, TOP, { origin: 'http://example.com', url: 'http://example.com/' });
  assert.equal((await b.send({ k: 'webauthn', request: CREATE }, insecure)).err.code, 'browser');
  const frame = Object.assign({}, TOP, { frameId: 3, origin: 'https://widget.example', url: 'https://widget.example/' });
  assert.equal((await b.send({ k: 'webauthn', request: CREATE, cross: true }, frame)).err.code, 'browser');
  const tabSays = Object.assign({}, frame, { tab: { id: 7, url: 'https://example.com/' } });
  assert.equal((await b.send({ k: 'webauthn', request: CREATE, cross: false }, tabSays)).err.code, 'browser');
  // A frame of the page's own site is answered.
  const same = Object.assign({}, TOP, { frameId: 3 });
  assert.ok((await b.send({ k: 'webauthn', request: CREATE, cross: false }, same)).ok);
  assert.equal(b.asked.length, 1);
});

test('one account signs in at once; several are chosen between, and the page gets only that one', async () => {
  const one = background({ host: () => ({ ok: { choices: [choice('paulus', 1)] } }) });
  const single = await one.send({ k: 'webauthn', doc: 'd', request: GET }, TOP);
  const data = '{"type":"webauthn.get","challenge":"AAEC","origin":"https://example.com","crossOrigin":false}';
  assert.deepEqual(one.asked[0], { id: one.asked[0].id, who: { app: 'Chrome' }, op: 'passkey_get', origin: 'https://example.com', rp_id: 'example.com', client_data_hash: sha(data), allow: [], conditional: false });
  assert.deepEqual(single, { ok: { credential_id: 'id1', client_data_json: b64url(data), authenticator_data: 'ad1', signature: 'sig1', user_handle: 'uh1' } });
  assert.equal(one.toTabs.length, 0);

  const two = background({ host: () => ({ ok: { choices: [choice('paulus', 1), choice('work', 2)] } }), choose: () => ({ choice: 1 }) });
  const chosen = await two.send({ k: 'webauthn', doc: 'd', request: GET }, TOP);
  assert.equal(chosen.ok.credential_id, 'id2');
  assert.equal(chosen.ok.signature, 'sig2');
  assert.equal(JSON.stringify(chosen).indexOf('sig1'), -1);
  const shown = two.toTabs[0];
  assert.equal(shown.tab, 7);
  assert.deepEqual(shown.options, { frameId: 0, documentId: 'doc-a' });
  assert.deepEqual(shown.message, { what: 'passkey', site: 'example.com', items: [{ name: 'PAULUS', detail: 'paulus' }, { name: 'WORK', detail: 'work' }], browser: true, k: 'choose' });

  const other = background({ host: () => ({ ok: { choices: [choice('a', 1), choice('b', 2)] } }), choose: () => ({ browser: true }) });
  assert.equal((await other.send({ k: 'webauthn', request: GET }, TOP)).err.code, 'browser');
  const none = background({ host: () => ({ ok: { choices: [choice('a', 1), choice('b', 2)] } }), choose: () => ({ cancel: true }) });
  assert.equal((await none.send({ k: 'webauthn', request: GET }, TOP)).err.code, 'denied');
});

test('SpatiKey\'s refusals, as the page hears them', async () => {
  const cases = [['no_match', 'no_match'], ['excluded', 'excluded'], ['denied', 'denied'], ['timeout', 'timeout'],
    ['no_vault', 'browser'], ['not_same_lan', 'browser'], ['no_approver', 'browser'], ['bad_request', 'browser'], ['failed', 'browser']];
  for (const [code, heard] of cases) {
    const b = background({ host: () => ({ err: { code, message: 'm' } }) });
    assert.equal((await b.send({ k: 'webauthn', request: GET }, TOP)).err.code, heard, code);
  }
  const gone = background({ host: () => 'gone' });
  const answer = await gone.send({ k: 'webauthn', request: CREATE }, TOP);
  assert.equal(answer.err.code, 'browser');
  assert.match(answer.err.message, /not found/);
});

test('one request at a time in a tab, and an abort lets the page go at once', async () => {
  let release;
  const b = background({ host: (m) => (m.op === 'passkey_get' ? null : { ok: CREATED }), choose: () => new Promise((r) => { release = r; }) });
  const first = b.send({ k: 'webauthn', doc: 'd', request: GET }, TOP);
  await new Promise((r) => setImmediate(r));
  assert.equal((await b.send({ k: 'webauthn', doc: 'd', request: Object.assign({}, CREATE, { id: 9 }) }, TOP)).err.code, 'pending');
  await b.send({ k: 'abort', doc: 'd', id: GET.id }, TOP);
  // The aborted one no longer holds the tab.
  assert.ok((await b.send({ k: 'webauthn', doc: 'd', request: Object.assign({}, CREATE, { id: 10 }) }, TOP)).ok);
  assert.equal(release, undefined);
  void first;
});

test('an abort while the person chooses takes the chooser away', async () => {
  const b = background({ host: () => ({ ok: { choices: [choice('a', 1), choice('b', 2)] } }), choose: () => new Promise(() => {}) });
  const asked = b.send({ k: 'webauthn', doc: 'd', request: GET }, TOP);
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(b.toTabs[0].message.k, 'choose');
  await b.send({ k: 'abort', doc: 'd', id: GET.id }, TOP);
  assert.equal(b.toTabs[1].message.k, 'close');
  assert.deepEqual(b.toTabs[1].options, { frameId: 0, documentId: 'doc-a' });
  void asked;
});

test('filling: the browser\'s origin, one login at once, several chosen between', async () => {
  const one = background({ host: () => ({ ok: { logins: [{ id: 'e1', title: 'Example', username: 'paulus', password: 'hunter2', totp: '123456' }] } }) });
  assert.deepEqual(await one.send({ k: 'fill' }, TOP), { login: { username: 'paulus', password: 'hunter2', totp: '123456' } });
  assert.deepEqual(one.asked[0], { id: one.asked[0].id, who: { app: 'Chrome' }, op: 'fill', origin: 'https://example.com' });
  const two = background({
    host: () => ({ ok: { logins: [{ id: 'e1', title: 'Example', username: 'paulus', password: 'one' }, { id: 'e2', title: '', username: 'work', password: 'two' }] } }),
    choose: (m) => { assert.deepEqual(m.items, [{ name: 'Example', detail: 'paulus' }, { name: 'work', detail: 'work' }]); return { choice: 1 }; }
  });
  assert.deepEqual(await two.send({ k: 'fill' }, TOP), { login: { username: 'work', password: 'two', totp: null } });
  const cancel = background({ host: () => ({ ok: { logins: [{ username: 'a', password: 'one' }, { username: 'b', password: 'two' }] } }), choose: () => ({ cancel: true }) });
  assert.deepEqual(await cancel.send({ k: 'fill' }, TOP), { cancel: true });
  const empty = background({ host: () => ({ ok: { logins: [] } }) });
  assert.deepEqual(await empty.send({ k: 'fill' }, TOP), { none: true });
  const far = background({ host: () => ({ err: { code: 'not_same_lan', message: 'no' } }) });
  assert.deepEqual(await far.send({ k: 'fill' }, TOP), { err: { code: 'not_same_lan', message: 'no' } });
  const insecure = background();
  assert.equal((await insecure.send({ k: 'fill' }, Object.assign({}, TOP, { origin: 'http://example.com' }))).err.code, 'insecure');
  assert.equal(insecure.asked.length, 0);
});

test('saving: a login sent is offered in the top frame, kept for the next page, saved on yes', async () => {
  const b = background({ host: (m) => (m.op === 'save' ? { ok: { id: 'new' } } : null) });
  const frame = Object.assign({}, TOP, { frameId: 4, origin: 'https://login.example.com', url: 'https://login.example.com/' });
  await b.send({ k: 'submitted', username: 'paulus', password: 's3cret', kind: 'password' }, frame);
  await new Promise((r) => setImmediate(r));
  const offered = b.toTabs[0];
  assert.deepEqual(offered.options, { frameId: 0 });
  assert.equal(offered.message.k, 'offer');
  assert.deepEqual(Object.keys(offered.message.offer).sort(), ['kind', 'site', 'token', 'username']);
  assert.equal(offered.message.offer.site, 'login.example.com');
  assert.equal(JSON.stringify(offered).indexOf('s3cret'), -1, 'the password stays in the background');
  // The next page in the tab asks, and is offered the same.
  const next = await b.send({ k: 'offers' }, TOP);
  assert.equal(next.offer.token, offered.message.offer.token);
  assert.deepEqual(await b.send({ k: 'offers' }, Object.assign({}, TOP, { frameId: 2 })), {});
  assert.deepEqual(await b.send({ k: 'save', token: 'wrong' }, TOP), { err: { code: 'gone', message: 'That login is no longer here.' } });
  assert.deepEqual(await b.send({ k: 'save', token: next.offer.token }, TOP), { ok: true });
  const saved = b.asked.find((m) => m.op === 'save');
  assert.deepEqual(saved, { id: saved.id, who: { app: 'Chrome' }, op: 'save', origin: 'https://login.example.com', title: '', username: 'paulus', password: 's3cret' });
  // Once answered it is gone.
  assert.deepEqual(await b.send({ k: 'offers' }, TOP), {});
  assert.equal((await b.send({ k: 'save', token: next.offer.token }, TOP)).err.code, 'gone');
});

test('saving: "not now", a closed tab, and pages that are not secure', async () => {
  const b = background({ host: () => null });
  await b.send({ k: 'submitted', username: 'a', password: 'one' }, TOP);
  const { offer } = await b.send({ k: 'offers' }, TOP);
  await b.send({ k: 'dismiss', token: offer.token }, TOP);
  assert.deepEqual(await b.send({ k: 'offers' }, TOP), {});
  await b.send({ k: 'submitted', username: 'a', password: 'two' }, TOP);
  b.closeTab(7);
  assert.deepEqual(await b.send({ k: 'offers' }, TOP), {});
  await b.send({ k: 'submitted', username: 'a', password: 'three' }, Object.assign({}, TOP, { origin: 'http://example.com' }));
  assert.deepEqual(await b.send({ k: 'offers' }, TOP), {});
  await b.send({ k: 'submitted', username: 'a', password: '' }, TOP);
  assert.deepEqual(await b.send({ k: 'offers' }, TOP), {});
});

test('who is asking: the browser, by name', () => {
  assert.equal(background().helpers.browserName(), 'Chrome');
  assert.equal(background({ brands: ['Brave', 'Chromium'] }).helpers.browserName(), 'Brave');
  assert.equal(background({ brands: ['Microsoft Edge', 'Chromium'] }).helpers.browserName(), 'Edge');
  assert.equal(background({ brands: ['Chromium'] }).helpers.browserName(), 'Chromium');
  assert.equal(background({ userAgent: 'Mozilla/5.0 (Macintosh) Gecko/20100101 Firefox/156.0', brands: [] }).helpers.browserName(), 'Firefox');
});

test('messages from anything but this extension\'s content scripts are not answered', async () => {
  const b = background({ host: () => ({ ok: { logins: [] } }) });
  assert.equal(await b.send({ k: 'fill' }, { id: 'someone-else', tab: { id: 1 }, frameId: 0, origin: 'https://example.com' }), undefined);
  assert.equal(await b.send({ k: 'fill' }, { frameId: 0, origin: 'https://example.com' }), undefined);
  assert.equal(await b.send({ k: 'nonsense' }, TOP), undefined);
  assert.equal(b.asked.length, 0);
});
