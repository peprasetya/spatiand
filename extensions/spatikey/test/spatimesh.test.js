// SpatiMesh's half in a real browser: headless Chrome stands in for WPE. webauthn.js runs in the
// page's world with the browser's own WebAuthn taken away, and forms.js + cred-world.js in an
// isolated world named "spatimesh-cred", with a stand-in for WebKit's message handler with reply
// (webkit.messageHandlers.spatimeshCred) whose posts come to this test, which plays SpatiMesh's
// window: it reads what each frame says, and answers. Typing and clicking are a person's (trusted
// input events), through the DevTools protocol.
//
//   node --test extensions/spatikey/test/*.test.js
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { serve, chromeThere, CHROME, ROOT, REPO } = require('./harness');
const { launch, openTab, sleep, until } = require('./cdp');

const skip = chromeThere() ? false : `no Chrome at ${CHROME}`;
const read = (...parts) => fs.readFileSync(path.join(...parts), 'utf8');

// The page's world: no WebAuthn of the browser's own, then webauthn.js.
const PAGE_WORLD = `
  ['PublicKeyCredential', 'Credential', 'PasswordCredential', 'FederatedCredential', 'IdentityCredential', 'OTPCredential',
   'DigitalCredential', 'CredentialsContainer', 'AuthenticatorResponse', 'AuthenticatorAttestationResponse',
   'AuthenticatorAssertionResponse'].forEach(function (name) { delete window[name]; });
  delete Navigator.prototype.credentials;
` + read(ROOT, 'src', 'webauthn.js');

// The isolated world: WebKit's handler with reply, played through the binding spatimeshCredPost;
// the test answers post n by calling __deliver(n, value).
const FAKE_WEBKIT = `
(function () {
  var waiting = {}, next = 1;
  window.__deliver = function (n, value, error) {
    var w = waiting[n];
    if (!w) return;
    delete waiting[n];
    if (error) w.reject(new Error(error)); else w.resolve(value);
  };
  window.webkit = { messageHandlers: { spatimeshCred: { postMessage: function (json) {
    var n = next++;
    return new Promise(function (resolve, reject) {
      waiting[n] = { resolve: resolve, reject: reject };
      window.spatimeshCredPost(JSON.stringify({ n: n, json: json }));
    });
  } } } };
})();
`;
const CRED_WORLD = FAKE_WEBKIT + read(ROOT, 'src', 'forms.js') + '\n' + read(REPO, 'crates', 'spatimesh', 'src', 'credentials', 'cred-world.js');

const b64 = (text) => Buffer.from(text).toString('base64url');
const CREATED = { credential_id: b64('cred-1'), client_data_json: b64('{"type":"webauthn.create"}'), attestation_object: b64('att'), authenticator_data: b64('ad'), public_key: b64('spki'), alg: -7, transports: ['internal'] };
const ASSERTED = { credential_id: b64('cred-2'), client_data_json: b64('{"type":"webauthn.get"}'), authenticator_data: b64('ad2'), signature: b64('sig'), user_handle: b64('uh') };

test('SpatiMesh: cred-world.js stamps, relays and fills, in its own world', { skip, timeout: 120000 }, async () => {
  const server = await serve();
  const browser = await launch();
  try {
    const { cdp } = browser;
    const tab = await openTab(cdp);
    const top = `http://localhost:${server.port}`, other = `http://127.0.0.1:${server.port}`;
    // Every frame's scripts, in the tab and in each frame Chrome runs apart (another site's frame
    // has a process, and a target, of its own): set up before it runs anything.
    const sessions = new Set([tab.sessionId]);
    const contexts = new Map();
    const posts = [];
    async function prepare(sessionId) {
      const send = (method, params) => cdp.send(method, params, sessionId);
      await send('Page.enable');
      await send('Runtime.enable');
      await send('Runtime.addBinding', { name: 'spatimeshCredPost', executionContextName: 'spatimesh-cred' });
      await send('Page.addScriptToEvaluateOnNewDocument', { source: PAGE_WORLD });
      await send('Page.addScriptToEvaluateOnNewDocument', { source: CRED_WORLD, worldName: 'spatimesh-cred' });
      await send('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: true, flatten: true });
    }
    cdp.on((m) => {
      if (!sessions.has(m.sessionId)) return;
      if (m.method === 'Target.attachedToTarget') {
        const child = m.params.sessionId;
        sessions.add(child);
        prepare(child).then(() => cdp.send('Runtime.runIfWaitingForDebugger', {}, child));
      } else if (m.method === 'Runtime.executionContextCreated') {
        const c = m.params.context;
        contexts.set(`${m.sessionId}:${c.id}`, { sessionId: m.sessionId, id: c.id, name: c.name, origin: c.origin, isDefault: !!(c.auxData && c.auxData.isDefault) });
      } else if (m.method === 'Runtime.bindingCalled' && m.params.name === 'spatimeshCredPost') {
        const { n, json } = JSON.parse(m.params.payload);
        posts.push({ n, sessionId: m.sessionId, contextId: m.params.executionContextId, message: JSON.parse(json), answered: false });
      }
    });
    async function evaluateIn(sessionId, contextId, expression) {
      const { result, exceptionDetails } = await cdp.send('Runtime.evaluate', { expression, contextId, awaitPromise: true, returnByValue: true }, sessionId);
      if (exceptionDetails) throw new Error(exceptionDetails.text);
      return result.value;
    }
    // The window's side of a post: its answer, as WebKit's reply would give it.
    async function answer(post, value, error) {
      post.answered = true;
      await evaluateIn(post.sessionId, post.contextId, `__deliver(${post.n}, ${JSON.stringify(value)}${error ? ', ' + JSON.stringify(error) : ''})`);
    }
    const waitPost = (match, what) => until(() => posts.find((p) => !p.answered && match(p.message)), what);
    const inPage = (origin) => [...contexts.values()].find((c) => c.isDefault && c.origin === origin);

    await prepare(tab.sessionId);
    await tab.navigate(server.url('/pages/spatimesh.html'));

    // Each document keeps a "ready" waiting, stamped with what its world knows of it.
    const topReady = await waitPost((m) => m.t === 'ready' && m.main === true, 'the top frame\'s ready');
    // (The frame's first document is about:blank, its parent's origin; the page that loads into it
    // says its own.)
    const frameReady = await waitPost((m) => m.t === 'ready' && m.main === false && m.origin === other, 'the frame\'s ready');
    assert.equal(topReady.message.origin, top);
    assert.equal(topReady.message.top_origin, top);
    assert.deepEqual(topReady.message.ancestors, []);
    assert.match(topReady.message.frame, /^[0-9a-f]{16}$/);
    assert.equal(frameReady.message.origin, other);
    assert.equal(frameReady.message.top_origin, top, 'the top frame\'s origin, through ancestorOrigins');
    assert.deepEqual(frameReady.message.ancestors, [top]);
    assert.notEqual(frameReady.message.frame, topReady.message.frame);
    assert.equal(contexts.get(`${topReady.sessionId}:${topReady.contextId}`).name, 'spatimesh-cred', 'posted from the isolated world');
    assert.equal(await tab.evaluate('typeof window.webkit + typeof window.__spatikeyCredWorld + typeof SpatiKeyForms'), 'undefinedundefinedundefined', 'nothing of it in the page\'s world');

    // create(): stamped, answered (as a string), and a PublicKeyCredential in the page.
    const created = tab.evaluate(`navigator.credentials.create({ publicKey: { rp: { name: 'Example' }, user: { id: new Uint8Array([1]), name: 'paulus', displayName: 'Paulus' },
      challenge: new Uint8Array([1, 2, 3]), pubKeyCredParams: [{ type: 'public-key', alg: -7 }] } })
      .then((c) => ({ pkc: c instanceof PublicKeyCredential, att: c.response instanceof AuthenticatorAttestationResponse, id: c.id, data: new TextDecoder().decode(c.response.clientDataJSON) }))`);
    const create = await waitPost((m) => m.t === 'create', 'the create request');
    assert.deepEqual(Object.assign({}, create.message, { id: 0 }), {
      t: 'create', challenge: 'AQID', rp: { name: 'Example' }, user: { id: 'AQ', name: 'paulus', displayName: 'Paulus' }, algs: [-7], exclude: [], timeout: 300000, id: 0,
      origin: top, main: true, frame: topReady.message.frame, top_origin: top, ancestors: []
    });
    await answer(create, JSON.stringify({ ok: CREATED }));
    assert.deepEqual(await created, { pkc: true, att: true, id: CREATED.credential_id, data: '{"type":"webauthn.create"}' });

    // A page that says it is someone else is not believed.
    await tab.evaluate(`document.dispatchEvent(new CustomEvent('spatikey-request', { detail: JSON.stringify({ id: 999, t: 'get', challenge: 'AA', allow: [],
      conditional: false, timeout: 30000, origin: 'https://evil.example', main: false, frame: 'forged', top_origin: 'https://evil.example', ancestors: ['https://evil.example'] }) }))`);
    const forged = await waitPost((m) => m.id === 999, 'the forged request');
    assert.equal(forged.message.origin, top);
    assert.equal(forged.message.main, true);
    assert.equal(forged.message.frame, topReady.message.frame);
    assert.deepEqual(forged.message.ancestors, []);
    await answer(forged, { err: { code: 'denied', message: 'no' } });

    // A refusal from the window, as an error of WebKit's reply.
    const refused = tab.evaluate(`navigator.credentials.get({ publicKey: { challenge: new Uint8Array([9]) } }).then(() => 'resolved', (e) => e.name)`);
    const toRefuse = await waitPost((m) => m.t === 'get' && m.challenge === 'CQ', 'the get to refuse');
    await answer(toRefuse, null, 'the window said no');
    assert.equal(await refused, 'NotAllowedError');

    // A conditional get: its own post is answered with nothing, and the answer comes through the
    // frame's ready once the person has chosen.
    const conditional = tab.evaluate(`navigator.credentials.get({ publicKey: { challenge: new Uint8Array([1]) }, mediation: 'conditional' }).then((c) => c.id)`);
    const get = await waitPost((m) => m.t === 'get' && m.conditional === true, 'the conditional get');
    assert.equal(get.message.timeout, 0);
    await answer(get, '{}');
    await sleep(100);
    const readyNow = posts.find((p) => !p.answered && p.message.t === 'ready' && p.message.main);
    await answer(readyNow, { conditional: { id: get.message.id, ok: ASSERTED } });
    assert.equal(await conditional, ASSERTED.credential_id);
    await waitPost((m) => m.t === 'ready' && m.main === true, 'a new ready after the answer');

    // A login field takes the focus: the window hears it.
    await tab.clickOn('#username');
    const fields = await waitPost((m) => m.t === 'fields' && m.kind === 'username', 'the fields report');
    assert.equal(fields.message.webauthn, true);
    assert.equal(fields.message.origin, top);
    assert.equal(typeof fields.message.field, 'number');
    await tab.clickOn('#password');
    await waitPost((m) => m.t === 'fields' && m.kind === 'password', 'the password field\'s report');

    // The window fills it, through the ready.
    const toFill = await waitPost((m) => m.t === 'ready' && m.main === true, 'a ready to fill with');
    await answer(toFill, { fill: { username: 'paulus', password: 'hunter2' } });
    await until(() => tab.evaluate(`document.querySelector('#password').value === 'hunter2'`), 'the password filled');
    assert.equal(await tab.evaluate(`document.querySelector('#username').value`), 'paulus');
    const events = await tab.evaluate('window.events');
    for (const [type, id, value] of [['input', 'username', 'paulus'], ['change', 'username', 'paulus'], ['input', 'password', 'hunter2'], ['change', 'password', 'hunter2']]) {
      assert.ok(events.some((e) => e[0] === type && e[1] === id && e[2] === value), `${type} on #${id}`);
    }
    // Signing in with what SpatiKey filled offers nothing to save.
    await tab.clickOn('#signin');
    await until(() => tab.evaluate('window.signedIn === 1'), 'the login form sent');
    await sleep(200);
    assert.equal(posts.filter((p) => p.message.t === 'submitted').length, 0);

    // A sign-up the person types is reported when it is sent.
    await tab.clickOn('#new-user');
    await tab.type('new@example.com');
    await tab.clickOn('#new-password');
    await tab.type('n3w-secret!');
    await tab.key('Enter');
    const submitted = await waitPost((m) => m.t === 'submitted', 'the submitted report');
    assert.deepEqual([submitted.message.username, submitted.message.password, submitted.message.kind, submitted.message.origin, submitted.message.main],
      ['new@example.com', 'n3w-secret!', 'new-password', top, true]);
    assert.equal(await tab.evaluate('window.signedUp'), true);
    // A password a script sets is not one the person typed.
    await tab.evaluate(`document.querySelector('#password').value = 'set-by-script'; document.querySelector('#signin').click()`);
    await sleep(200);
    assert.equal(posts.filter((p) => p.message.t === 'submitted').length, 1);

    // The frame of another origin is filled in itself.
    const frameFill = await waitPost((m) => m.t === 'ready' && m.main === false && m.origin === other, 'the frame\'s ready');
    await answer(frameFill, { fill: { username: 'framed', password: 'in-the-frame' } });
    const frame = inPage(other);
    await until(() => evaluateIn(frame.sessionId, frame.id, `document.querySelector('#p').value === 'in-the-frame'`), 'the frame filled');
    assert.equal(await evaluateIn(frame.sessionId, frame.id, `document.querySelector('#u').value`), 'framed');
    assert.equal(await tab.evaluate(`document.querySelector('#username').value`), 'paulus', 'the top frame untouched');

    // {stop:true}: that frame sends no more.
    const last = await waitPost((m) => m.t === 'ready' && m.main === true, 'a ready to stop');
    const count = posts.length;
    await answer(last, { stop: true });
    await sleep(300);
    assert.equal(posts.slice(count).filter((p) => p.message.t === 'ready' && p.message.main).length, 0);
  } finally {
    await browser.stop();
    await server.close();
  }
});
