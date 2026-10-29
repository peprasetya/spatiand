// webauthn.js in node: base64url both ways, what the page's options become as requests, what
// replies become as credentials, errors, aborts and timeouts -- in polyfill mode, and in wrapper
// mode against stand-ins for a browser's own classes. The real browser test is browser.test.js.
//
//   node --test extensions/spatikey/test/*.test.js
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const SOURCE = fs.readFileSync(path.join(__dirname, '..', 'src', 'webauthn.js'), 'utf8');

// A page for webauthn.js: a document to send events on, a navigator, and a clock the test moves.
// `before` runs in the page first (stand-ins for a browser's own WebAuthn, a library's mischief).
function page({ before = '', secure = true } = {}) {
  const document = new EventTarget();
  const timers = new Map();
  let nextTimer = 1;
  const context = {
    module: { exports: {} },
    document,
    isSecureContext: secure,
    EventTarget, Event, CustomEvent, DOMException, AbortController, AbortSignal, TextEncoder, TextDecoder, console,
    setTimeout: (fn, ms) => { const id = nextTimer++; timers.set(id, { fn, ms }); return id; },
    clearTimeout: (id) => { timers.delete(id); }
  };
  vm.createContext(context);
  vm.runInContext('var window = this; function Navigator() {} var navigator = Object.create(Navigator.prototype);', context);
  if (before) vm.runInContext(before, context);
  vm.runInContext(SOURCE, context, { filename: 'webauthn.js' });
  const sent = [];
  let answer = null;
  // The relay: every request is kept, and answered by `answer(request)` when it returns a reply.
  document.addEventListener('spatikey-request', (ev) => {
    const request = JSON.parse(ev.detail);
    sent.push(request);
    const reply = answer ? answer(request) : null;
    if (reply) setImmediate(() => say(Object.assign({ id: request.id }, reply)));
  });
  function say(reply) {
    document.dispatchEvent(new CustomEvent('spatikey-reply', { detail: JSON.stringify(reply) }));
  }
  return {
    context, sent, timers, say,
    helpers: context.module.exports,
    answer(fn) { answer = fn; },
    run(code) { return vm.runInContext(code, context); },
    fire(ms) {
      for (const [id, t] of timers) if (t.ms === ms) { timers.delete(id); t.fn(); return true; }
      return false;
    }
  };
}

const enc = (text) => Buffer.from(text).toString('base64url');
const bytes = (...b) => new Uint8Array(b);

const CREATED = {
  credential_id: enc('credential-1'),
  client_data_json: enc('{"type":"webauthn.create"}'),
  attestation_object: enc('attestation'),
  authenticator_data: enc('authenticator-data'),
  public_key: enc('spki'),
  alg: -7,
  transports: ['internal', 'hybrid', 'internal']
};

const ASSERTED = {
  credential_id: enc('credential-1'),
  client_data_json: enc('{"type":"webauthn.get"}'),
  authenticator_data: enc('authenticator-data'),
  signature: enc('signature'),
  user_handle: enc('user-1')
};

const CREATE_OPTIONS = `({
  rp: { id: 'example.com', name: 'Example' },
  user: { id: new Uint8Array([1, 2, 3, 4]), name: 'paulus@example.com', displayName: 'Paulus' },
  challenge: new Uint8Array([0xfb, 0xff, 0xbf, 0x00, 0x01]).buffer,
  pubKeyCredParams: [{ type: 'public-key', alg: -8 }, { type: 'public-key', alg: -7 }, { type: 'other', alg: -1 }],
  excludeCredentials: [{ type: 'public-key', id: new Uint8Array([9, 9]) }, { type: 'other', id: new Uint8Array([8]) }],
  timeout: 60000,
  extensions: { credProps: true }
})`;

test('base64url: RFC 4648 vectors, both ways', () => {
  const { toBase64url, fromBase64url } = page().helpers;
  const vectors = [['', ''], ['f', 'Zg'], ['fo', 'Zm8'], ['foo', 'Zm9v'], ['foob', 'Zm9vYg'], ['fooba', 'Zm9vYmE'], ['foobar', 'Zm9vYmFy']];
  for (const [plain, coded] of vectors) {
    assert.equal(toBase64url(new Uint8Array(Buffer.from(plain))), coded);
    assert.equal(Buffer.from(fromBase64url(coded)).toString(), plain);
  }
  // The two characters base64url has of its own.
  assert.equal(toBase64url(bytes(0xfb, 0xff, 0xbf)), '-_-_');
  assert.deepEqual([...fromBase64url('-_-_')], [0xfb, 0xff, 0xbf]);
});

test('base64url: padding is read, malformed text is not', () => {
  const { fromBase64url } = page().helpers;
  assert.deepEqual([...fromBase64url('Zg==')], [0x66]);
  assert.deepEqual([...fromBase64url('Zm8=')], [0x66, 0x6f]);
  for (const bad of ['Zg=', 'Z', 'Zm9v+', 'Zm9v/A', 'Zm 9v', 'Zg===', 'éA', 42, null, undefined]) {
    assert.equal(fromBase64url(bad), null, String(bad));
  }
});

test('base64url: agrees with Buffer for every length up to 300', () => {
  const { toBase64url, fromBase64url } = page().helpers;
  for (let n = 0; n <= 300; n++) {
    const data = new Uint8Array(n).map((_, i) => (i * 37 + n * 11) & 255);
    const coded = toBase64url(data);
    assert.equal(coded, Buffer.from(data).toString('base64url'));
    assert.deepEqual([...fromBase64url(coded)], [...data]);
    assert.deepEqual([...fromBase64url(Buffer.from(data).toString('base64').replace(/\+/g, '-').replace(/\//g, '_'))], [...data]);
  }
});

test('BufferSource: buffers and views of any frame, copied as WebIDL says', () => {
  const { bytesOf } = page().helpers;
  const buffer = new Uint8Array([1, 2, 3, 4, 5]).buffer;
  const copy = bytesOf(buffer);
  new Uint8Array(buffer)[0] = 99;
  assert.deepEqual([...copy], [1, 2, 3, 4, 5]);
  assert.deepEqual([...bytesOf(new Uint8Array(buffer, 1, 3))], [2, 3, 4]);
  assert.deepEqual([...bytesOf(new DataView(buffer, 3))], [4, 5]);
  assert.deepEqual([...bytesOf(new Uint16Array([0x0201]))], [1, 2]);
  for (const bad of [null, undefined, 'abc', [1, 2], { byteLength: 2 }]) assert.equal(bytesOf(bad), null);
});

test('create(): the page\'s options as a request', () => {
  const p = page();
  const request = p.helpers.createRequest(p.run(CREATE_OPTIONS));
  assert.deepEqual(JSON.parse(JSON.stringify(request)), {
    t: 'create',
    challenge: '-_-_AAE',
    rp: { name: 'Example', id: 'example.com' },
    user: { id: 'AQIDBA', name: 'paulus@example.com', displayName: 'Paulus' },
    algs: [-8, -7],
    exclude: ['CQk'],
    timeout: 60000
  });
  // No list means ES256 or RS256; no rp id is the relay's to fill in, from the origin it knows.
  const bare = p.helpers.createRequest(p.run(`({ rp: { name: 'X' }, user: { id: new Uint8Array([1]), name: 'a', displayName: '' },
    challenge: new Uint8Array(16), pubKeyCredParams: [] })`));
  assert.deepEqual([...bare.algs], [-7, -257]);
  assert.equal(bare.rp.id, undefined);
  assert.equal(bare.timeout, 300000);
});

test('create(): what WebIDL refuses is a TypeError', () => {
  const p = page();
  const cases = [
    `({ user: { id: new Uint8Array(1), name: 'a', displayName: 'a' }, challenge: new Uint8Array(1), pubKeyCredParams: [] })`,
    `({ rp: {}, user: { id: new Uint8Array(1), name: 'a', displayName: 'a' }, challenge: new Uint8Array(1), pubKeyCredParams: [] })`,
    `({ rp: { name: 'x' }, user: { id: new Uint8Array(1), name: 'a' }, challenge: new Uint8Array(1), pubKeyCredParams: [] })`,
    `({ rp: { name: 'x' }, user: { id: new Uint8Array(65), name: 'a', displayName: 'a' }, challenge: new Uint8Array(1), pubKeyCredParams: [] })`,
    `({ rp: { name: 'x' }, user: { id: new Uint8Array(0), name: 'a', displayName: 'a' }, challenge: new Uint8Array(1), pubKeyCredParams: [] })`,
    `({ rp: { name: 'x' }, user: { id: 'abc', name: 'a', displayName: 'a' }, challenge: new Uint8Array(1), pubKeyCredParams: [] })`,
    `({ rp: { name: 'x' }, user: { id: new Uint8Array(1), name: 'a', displayName: 'a' }, challenge: 'not a buffer', pubKeyCredParams: [] })`,
    `({ rp: { name: 'x' }, user: { id: new Uint8Array(1), name: 'a', displayName: 'a' }, challenge: new Uint8Array(1) })`,
    `({ rp: { name: 'x' }, user: { id: new Uint8Array(1), name: 'a', displayName: 'a' }, challenge: new Uint8Array(1), pubKeyCredParams: [{ type: 'public-key' }] })`
  ];
  for (const options of cases) {
    assert.throws(() => p.helpers.createRequest(p.run(options)), (e) => e.name === 'TypeError', options);
  }
});

test('timeouts: half a minute at least, ten at most', () => {
  const { timeoutOf } = page().helpers;
  assert.equal(timeoutOf(undefined), 300000);
  assert.equal(timeoutOf(1000), 30000);
  assert.equal(timeoutOf(NaN), 30000);
  assert.equal(timeoutOf(120000.7), 120000);
  assert.equal(timeoutOf(1e9), 600000);
});

test('get(): the page\'s options as a request', () => {
  const p = page();
  const modal = p.helpers.getRequest(p.run(`({ challenge: new Uint8Array([1, 2, 3]), rpId: 'example.com',
    allowCredentials: [{ type: 'public-key', id: new Uint8Array([7]), transports: ['internal'] }], timeout: 45000 })`), 'optional');
  assert.deepEqual(JSON.parse(JSON.stringify(modal)), { t: 'get', challenge: 'AQID', allow: ['Bw'], conditional: false, timeout: 45000, rpId: 'example.com' });
  const conditional = p.helpers.getRequest(p.run(`({ challenge: new Uint8Array([1]) })`), 'conditional');
  assert.deepEqual(JSON.parse(JSON.stringify(conditional)), { t: 'get', challenge: 'AQ', allow: [], conditional: true, timeout: 0 });
  assert.throws(() => p.helpers.getRequest(p.run('({})'), undefined), (e) => e.name === 'TypeError');
});

test('a library\'s Array.prototype.toJSON does not reach what is sent', async () => {
  const p = page({ before: '' });
  p.run(`Array.prototype.toJSON = function () { return 'broken'; };`);
  p.answer(() => ({ ok: CREATED }));
  await p.run(`navigator.credentials.create({ publicKey: ${CREATE_OPTIONS} })`);
  assert.deepEqual(p.sent[0].algs, [-8, -7]);
  assert.deepEqual(p.sent[0].exclude, ['CQk']);
});

test('polyfill: WebAuthn is defined where the engine has none, in secure contexts only', () => {
  const p = page();
  for (const name of ['Credential', 'PublicKeyCredential', 'AuthenticatorResponse', 'AuthenticatorAttestationResponse', 'AuthenticatorAssertionResponse', 'CredentialsContainer']) {
    assert.equal(typeof p.context[name], 'function', name);
    assert.throws(() => p.run(`new ${name}()`), (e) => e.name === 'TypeError');
  }
  assert.equal(p.run('Object.getPrototypeOf(PublicKeyCredential)'), p.context.Credential);
  assert.equal(p.run('navigator.credentials instanceof CredentialsContainer'), true);
  assert.equal(p.run('navigator.credentials === navigator.credentials'), true);
  assert.equal(p.run('Object.prototype.hasOwnProperty.call(navigator, "credentials")'), false);
  assert.equal(p.run('Object.prototype.toString.call(navigator.credentials)'), '[object CredentialsContainer]');
  assert.equal(p.run('Object.keys(window).indexOf("PublicKeyCredential")'), -1);
  const insecure = page({ secure: false });
  assert.equal(insecure.context.PublicKeyCredential, undefined);
  assert.equal(insecure.run('navigator.credentials'), undefined);
});

test('polyfill: create() asks the relay and makes a PublicKeyCredential of the answer', async () => {
  const p = page();
  p.answer(() => ({ ok: CREATED }));
  const credential = await p.run(`navigator.credentials.create({ publicKey: ${CREATE_OPTIONS} })`);
  assert.equal(p.sent.length, 1);
  assert.equal(p.sent[0].t, 'create');
  assert.equal(typeof p.sent[0].id, 'number');
  assert.equal(p.sent[0].origin, undefined, 'the page never says its origin');
  p.context.credential = credential;
  assert.equal(p.run('credential instanceof PublicKeyCredential && credential instanceof Credential'), true);
  assert.equal(p.run('credential.response instanceof AuthenticatorAttestationResponse && credential.response instanceof AuthenticatorResponse'), true);
  assert.equal(p.run('Object.getOwnPropertyNames(credential).length + Object.getOwnPropertyNames(credential.response).length'), 0);
  assert.equal(credential.id, CREATED.credential_id);
  assert.equal(Buffer.from(credential.rawId).toString(), 'credential-1');
  assert.equal(credential.rawId, credential.rawId, 'the same buffer each time');
  assert.equal(credential.type, 'public-key');
  assert.equal(credential.authenticatorAttachment, 'platform');
  assert.equal(Buffer.from(credential.response.clientDataJSON).toString(), '{"type":"webauthn.create"}');
  assert.equal(Buffer.from(credential.response.attestationObject).toString(), 'attestation');
  assert.equal(Buffer.from(credential.response.getAuthenticatorData()).toString(), 'authenticator-data');
  assert.equal(Buffer.from(credential.response.getPublicKey()).toString(), 'spki');
  assert.equal(credential.response.getPublicKeyAlgorithm(), -7);
  assert.deepEqual([...credential.response.getTransports()], ['hybrid', 'internal']);
  assert.deepEqual(JSON.parse(JSON.stringify(credential.getClientExtensionResults())), { credProps: { rk: true } });
  assert.deepEqual(JSON.parse(JSON.stringify(credential)), {
    id: CREATED.credential_id,
    rawId: CREATED.credential_id,
    response: {
      clientDataJSON: CREATED.client_data_json,
      authenticatorData: CREATED.authenticator_data,
      transports: ['hybrid', 'internal'],
      publicKey: CREATED.public_key,
      publicKeyAlgorithm: -7,
      attestationObject: CREATED.attestation_object
    },
    authenticatorAttachment: 'platform',
    clientExtensionResults: { credProps: { rk: true } },
    type: 'public-key'
  });
  // The prototype's getters refuse anything this script did not make.
  assert.throws(() => p.run('Object.getOwnPropertyDescriptor(PublicKeyCredential.prototype, "rawId").get.call({})'), (e) => e.name === 'TypeError');
  assert.throws(() => p.run('AuthenticatorAssertionResponse.prototype.__lookupGetter__("signature").call(credential.response)'), (e) => e.name === 'TypeError');
});

test('polyfill: get() makes an assertion, its user handle null when SpatiKey gives none', async () => {
  const p = page();
  p.answer((request) => ({ ok: request.allow.length ? Object.assign({}, ASSERTED, { user_handle: '' }) : ASSERTED }));
  const credential = await p.run(`navigator.credentials.get({ publicKey: { challenge: new Uint8Array([5]), extensions: { appid: 'https://x' } } })`);
  p.context.credential = credential;
  assert.equal(p.run('credential instanceof PublicKeyCredential && credential.response instanceof AuthenticatorAssertionResponse'), true);
  assert.equal(Buffer.from(credential.response.signature).toString(), 'signature');
  assert.equal(Buffer.from(credential.response.authenticatorData).toString(), 'authenticator-data');
  assert.equal(Buffer.from(credential.response.userHandle).toString(), 'user-1');
  assert.deepEqual(JSON.parse(JSON.stringify(credential.getClientExtensionResults())), { appid: false });
  assert.deepEqual(JSON.parse(JSON.stringify(credential.toJSON())), {
    id: ASSERTED.credential_id,
    rawId: ASSERTED.credential_id,
    response: { clientDataJSON: ASSERTED.client_data_json, authenticatorData: ASSERTED.authenticator_data, signature: ASSERTED.signature, userHandle: ASSERTED.user_handle },
    authenticatorAttachment: 'platform',
    clientExtensionResults: { appid: false },
    type: 'public-key'
  });
  const second = await p.run(`navigator.credentials.get({ publicKey: { challenge: new Uint8Array([5]), allowCredentials: [{ type: 'public-key', id: new Uint8Array([1]) }] } })`);
  assert.equal(second.response.userHandle, null);
  assert.equal(second.toJSON().response.userHandle, undefined);
});

test('polyfill: refusals become the errors browsers give', async () => {
  const cases = [
    ['excluded', 'InvalidStateError'],
    ['security', 'SecurityError'],
    ['not_supported', 'NotSupportedError'],
    ['denied', 'NotAllowedError'],
    ['not_same_lan', 'NotAllowedError'],
    ['no_match', 'NotAllowedError'],
    ['browser', 'NotAllowedError'],
    ['something new', 'NotAllowedError']
  ];
  for (const [code, name] of cases) {
    const p = page();
    p.answer(() => ({ err: { code, message: 'SpatiKey said ' + code } }));
    await assert.rejects(p.run(`navigator.credentials.create({ publicKey: ${CREATE_OPTIONS} })`), (e) => {
      assert.equal(e.name, name, code);
      assert.ok(!/SpatiKey said/.test(e.message), 'the reason stays out of the page');
      return true;
    });
  }
  // An answer that cannot be read is a refusal like any other.
  const p = page();
  p.answer(() => ({ ok: { credential_id: '!!!' } }));
  await assert.rejects(p.run(`navigator.credentials.get({ publicKey: { challenge: new Uint8Array(1) } })`), (e) => e.name === 'NotAllowedError');
});

test('polyfill: what is refused without asking', async () => {
  const p = page();
  p.answer(() => ({ ok: CREATED }));
  const rejects = async (code, name) => assert.rejects(p.run(code), (e) => { assert.equal(e.name, name, code); return true; });
  await rejects(`navigator.credentials.create()`, 'NotSupportedError');
  await rejects(`navigator.credentials.get({ password: true })`, 'NotSupportedError');
  await rejects(`navigator.credentials.create({ publicKey: {} })`, 'TypeError');
  await rejects(`navigator.credentials.create({ publicKey: ${CREATE_OPTIONS}, signal: {} })`, 'TypeError');
  await rejects(`navigator.credentials.create({ publicKey: Object.assign(${CREATE_OPTIONS}, { pubKeyCredParams: [{ type: 'public-key', alg: -257 }] }) })`, 'NotSupportedError');
  await rejects(`navigator.credentials.create({ publicKey: Object.assign(${CREATE_OPTIONS}, { authenticatorSelection: { authenticatorAttachment: 'cross-platform' } }) })`, 'NotAllowedError');
  await rejects(`navigator.credentials.create({ publicKey: ${CREATE_OPTIONS}, mediation: 'conditional' })`, 'NotSupportedError');
  await rejects(`navigator.credentials.store({})`, 'NotSupportedError');
  assert.equal(await p.run(`navigator.credentials.preventSilentAccess()`), undefined);
  assert.equal(await p.run(`navigator.credentials.get({ publicKey: { challenge: new Uint8Array(1) }, mediation: 'silent' })`), null);
  assert.equal(p.sent.length, 0);
});

test('polyfill: an abort, before or during, is the signal\'s reason and stops the relay', async () => {
  const p = page();
  await assert.rejects(p.run(`(() => { const c = new AbortController(); c.abort(); return navigator.credentials.create({ publicKey: ${CREATE_OPTIONS}, signal: c.signal }); })()`),
    (e) => e.name === 'AbortError');
  assert.equal(p.sent.length, 0);
  p.run('var controller = new AbortController();');
  const asked = p.run(`navigator.credentials.get({ publicKey: { challenge: new Uint8Array(1) }, signal: controller.signal })`);
  assert.equal(p.sent.length, 1);
  p.run('controller.abort()');
  await assert.rejects(asked, (e) => e.name === 'AbortError');
  assert.deepEqual(p.sent[1], { id: p.sent[0].id, t: 'abort' });
  assert.equal(p.timers.size, 0, 'the timeout is let go');
  // A late answer goes nowhere.
  p.say({ id: p.sent[0].id, ok: ASSERTED });
  // An abort with a reason of the page's own.
  p.run('var other = new AbortController();');
  const again = p.run(`navigator.credentials.get({ publicKey: { challenge: new Uint8Array(1) }, signal: other.signal })`);
  p.run('other.abort(new Error("gone"))');
  await assert.rejects(again, (e) => e.message === 'gone');
});

test('polyfill: running out of time is NotAllowedError, and the relay is told to stop', async () => {
  const p = page();
  const asked = p.run(`navigator.credentials.create({ publicKey: Object.assign(${CREATE_OPTIONS}, { timeout: 45000 }) })`);
  assert.equal(p.fire(45000), true);
  await assert.rejects(asked, (e) => e.name === 'NotAllowedError');
  assert.deepEqual(p.sent[1], { id: p.sent[0].id, t: 'abort' });
  // A conditional get waits for the person however long it takes.
  p.run(`navigator.credentials.get({ publicKey: { challenge: new Uint8Array(1) }, mediation: 'conditional' })`);
  assert.equal(p.sent[2].conditional, true);
  assert.equal(p.timers.size, 0);
});

test('polyfill: the statics', async () => {
  const p = page();
  assert.equal(await p.run('PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable()'), true);
  assert.equal(await p.run('PublicKeyCredential.isConditionalMediationAvailable()'), true);
  assert.equal(await p.run('Credential.isConditionalMediationAvailable()'), false);
  const capabilities = await p.run('PublicKeyCredential.getClientCapabilities()');
  assert.equal(capabilities.conditionalGet, true);
  assert.equal(capabilities.passkeyPlatformAuthenticator, true);
  assert.equal(capabilities.hybridTransport, false);
  const creation = p.run(`PublicKeyCredential.parseCreationOptionsFromJSON({ rp: { name: 'X' }, user: { id: 'AQID', name: 'a', displayName: 'A' },
    challenge: '-_-_', pubKeyCredParams: [{ type: 'public-key', alg: -7 }], excludeCredentials: [{ type: 'public-key', id: 'Bw' }], timeout: 1 })`);
  assert.deepEqual([...new Uint8Array(creation.challenge)], [0xfb, 0xff, 0xbf]);
  assert.deepEqual([...new Uint8Array(creation.user.id)], [1, 2, 3]);
  assert.deepEqual([...new Uint8Array(creation.excludeCredentials[0].id)], [7]);
  assert.equal(creation.timeout, 1);
  assert.equal(creation.rp.name, 'X');
  const request = p.run(`PublicKeyCredential.parseRequestOptionsFromJSON({ challenge: 'AQ', allowCredentials: [{ type: 'public-key', id: 'Bw', transports: ['internal'] }], rpId: 'x.com' })`);
  assert.deepEqual([...new Uint8Array(request.challenge)], [1]);
  assert.deepEqual([...new Uint8Array(request.allowCredentials[0].id)], [7]);
  assert.deepEqual([...request.allowCredentials[0].transports], ['internal']);
  assert.throws(() => p.run(`PublicKeyCredential.parseRequestOptionsFromJSON({ challenge: 'a+b' })`), (e) => e.name === 'EncodingError');
  assert.throws(() => p.run(`PublicKeyCredential.parseRequestOptionsFromJSON({})`), (e) => e.name === 'TypeError');
  // What parse gives, create takes.
  p.answer(() => ({ ok: CREATED }));
  p.context.options = creation;
  await p.run('navigator.credentials.create({ publicKey: options })');
  assert.equal(p.sent[0].challenge, '-_-_');
  assert.deepEqual(p.sent[0].exclude, ['Bw']);
});

// Stand-ins for a browser's own WebAuthn: getters that refuse what the browser did not make, and
// create() and get() that say they were called.
const NATIVE = `
  var made = new WeakSet();
  function own(name) { return function () { if (!made.has(this)) throw new TypeError('Illegal invocation'); return 'native ' + name; }; }
  function Credential() { throw new TypeError('Illegal constructor'); }
  Credential.prototype = { get id() { return own('id').call(this); }, get type() { return own('type').call(this); } };
  function PublicKeyCredential() { throw new TypeError('Illegal constructor'); }
  PublicKeyCredential.prototype = Object.create(Credential.prototype, {
    rawId: { get: own('rawId') }, response: { get: own('response') }, authenticatorAttachment: { get: own('authenticatorAttachment') },
    getClientExtensionResults: { value: own('getClientExtensionResults') }, toJSON: { value: own('toJSON') } });
  Object.setPrototypeOf(PublicKeyCredential, Credential);
  PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable = function () { return Promise.resolve(false); };
  PublicKeyCredential.getClientCapabilities = function () { return Promise.resolve({ hybridTransport: true }); };
  PublicKeyCredential.parseRequestOptionsFromJSON = function () { return 'native parse'; };
  function AuthenticatorResponse() {}
  AuthenticatorResponse.prototype = { get clientDataJSON() { return own('clientDataJSON').call(this); } };
  function AuthenticatorAttestationResponse() {}
  AuthenticatorAttestationResponse.prototype = Object.create(AuthenticatorResponse.prototype, {
    attestationObject: { get: own('attestationObject') }, getTransports: { value: own('getTransports') } });
  function AuthenticatorAssertionResponse() {}
  AuthenticatorAssertionResponse.prototype = Object.create(AuthenticatorResponse.prototype, { signature: { get: own('signature') } });
  var nativeCalls = [];
  function CredentialsContainer() {}
  CredentialsContainer.prototype.create = function create(options) { nativeCalls.push(['create', options, this]); return Promise.resolve('native create'); };
  CredentialsContainer.prototype.get = function get(options) {
    nativeCalls.push(['get', options, this]);
    if (options && options.mediation === 'conditional') {
      return new Promise(function (resolve, reject) {
        window.nativeConditional = resolve;
        if (options.signal) options.signal.addEventListener('abort', function () { reject(options.signal.reason); });
      });
    }
    return Promise.resolve('native get');
  };
  Navigator.prototype.credentials = new CredentialsContainer();
`;

test('wrapper: answers are the browser\'s own classes, with their own getters', async () => {
  const p = page({ before: NATIVE });
  p.answer((request) => ({ ok: request.t === 'create' ? CREATED : ASSERTED }));
  const created = await p.run(`navigator.credentials.create({ publicKey: ${CREATE_OPTIONS} })`);
  p.context.created = created;
  assert.equal(p.run('created instanceof PublicKeyCredential && created instanceof Credential'), true);
  assert.equal(p.run('created.response instanceof AuthenticatorAttestationResponse && created.response instanceof AuthenticatorResponse'), true);
  assert.equal(created.id, CREATED.credential_id);
  assert.equal(Buffer.from(created.response.attestationObject).toString(), 'attestation');
  assert.equal(Buffer.from(created.response.clientDataJSON).toString(), '{"type":"webauthn.create"}');
  assert.deepEqual([...created.response.getTransports()], ['hybrid', 'internal']);
  assert.equal(created.response.getPublicKeyAlgorithm(), -7);
  assert.equal(created.toJSON().response.attestationObject, CREATED.attestation_object);
  assert.equal(p.run('Object.getOwnPropertyDescriptor(created, "rawId").get !== undefined'), true);
  const asserted = await p.run(`navigator.credentials.get({ publicKey: { challenge: new Uint8Array(1) } })`);
  p.context.asserted = asserted;
  assert.equal(p.run('asserted instanceof PublicKeyCredential && asserted.response instanceof AuthenticatorAssertionResponse'), true);
  assert.equal(Buffer.from(asserted.response.signature).toString(), 'signature');
  assert.equal(p.run('nativeCalls.length'), 0);
  // The statics: SpatiKey is a platform authenticator; the browser's own parse is kept.
  assert.equal(await p.run('PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable()'), true);
  const capabilities = await p.run('PublicKeyCredential.getClientCapabilities()');
  assert.equal(capabilities.hybridTransport, true);
  assert.equal(capabilities.passkeyPlatformAuthenticator, true);
  assert.equal(p.run('PublicKeyCredential.parseRequestOptionsFromJSON({})'), 'native parse');
  assert.equal(typeof p.run('PublicKeyCredential.parseCreationOptionsFromJSON'), 'function');
});

test('wrapper: the browser\'s own is asked when SpatiKey has nothing, or is told to step aside', async () => {
  const p = page({ before: NATIVE });
  p.answer((request) => ({ err: { code: request.t === 'get' ? 'no_match' : 'browser' } }));
  assert.equal(await p.run(`navigator.credentials.get(window.getOptions = { publicKey: { challenge: new Uint8Array(1) } })`), 'native get');
  assert.equal(await p.run(`navigator.credentials.create(window.createOptions = { publicKey: ${CREATE_OPTIONS} })`), 'native create');
  assert.equal(p.run('nativeCalls[0][1] === getOptions && nativeCalls[0][2] === navigator.credentials'), true);
  assert.equal(p.run('nativeCalls[1][1] === createOptions'), true);
  // Refusals that are the person's are the page's to hear, not the browser's to retry.
  p.answer(() => ({ err: { code: 'denied' } }));
  await assert.rejects(p.run(`navigator.credentials.get({ publicKey: { challenge: new Uint8Array(1) } })`), (e) => e.name === 'NotAllowedError');
  p.answer(() => ({ err: { code: 'excluded' } }));
  await assert.rejects(p.run(`navigator.credentials.create({ publicKey: ${CREATE_OPTIONS} })`), (e) => e.name === 'InvalidStateError');
  assert.equal(p.run('nativeCalls.length'), 2);
});

test('wrapper: what SpatiKey cannot do goes straight to the browser\'s own', async () => {
  const p = page({ before: NATIVE });
  p.answer(() => ({ ok: CREATED }));
  const straight = [
    `navigator.credentials.create({ publicKey: Object.assign(${CREATE_OPTIONS}, { pubKeyCredParams: [{ type: 'public-key', alg: -257 }] }) })`,
    `navigator.credentials.create({ publicKey: Object.assign(${CREATE_OPTIONS}, { hints: ['security-key'] }) })`,
    `navigator.credentials.create({ publicKey: Object.assign(${CREATE_OPTIONS}, { extensions: { prf: {} } }) })`,
    `navigator.credentials.create({ publicKey: {} })`,
    `navigator.credentials.create({ password: {} })`,
    `navigator.credentials.get({ publicKey: { challenge: new Uint8Array(1), allowCredentials: [{ type: 'public-key', id: new Uint8Array(1), transports: ['usb', 'nfc'] }] } })`,
    `navigator.credentials.get({ publicKey: { challenge: new Uint8Array(1) }, mediation: 'silent' })`,
    `navigator.credentials.get({ otp: { transport: ['sms'] } })`
  ];
  for (const code of straight) await p.run(code);
  assert.equal(p.run('nativeCalls.length'), straight.length);
  assert.equal(p.sent.length, 0);
});

test('wrapper: a conditional get waits on both, and the first answer stops the other', async () => {
  const p = page({ before: NATIVE });
  // The browser's own answers first: SpatiKey's wait is stopped.
  const first = p.run(`navigator.credentials.get({ publicKey: { challenge: new Uint8Array(1) }, mediation: 'conditional' })`);
  await new Promise((r) => setImmediate(r));
  assert.equal(p.sent.length, 1);
  assert.equal(p.sent[0].conditional, true);
  p.run('nativeConditional("native passkey")');
  assert.equal(await first, 'native passkey');
  assert.deepEqual(p.sent[1], { id: p.sent[0].id, t: 'abort' });
  // SpatiKey answers first: the browser's own is aborted.
  const second = p.run(`navigator.credentials.get({ publicKey: { challenge: new Uint8Array(1) }, mediation: 'conditional' })`);
  await new Promise((r) => setImmediate(r));
  p.say({ id: p.sent[2].id, ok: ASSERTED });
  const credential = await second;
  assert.equal(credential.id, ASSERTED.credential_id);
  assert.equal(p.run('nativeCalls[nativeCalls.length - 1][1].signal.aborted'), true);
  // SpatiKey has nothing: the browser's own goes on waiting, and answers.
  const third = p.run(`navigator.credentials.get({ publicKey: { challenge: new Uint8Array(1) }, mediation: 'conditional' })`);
  await new Promise((r) => setImmediate(r));
  p.say({ id: p.sent[3].id, err: { code: 'no_match' } });
  await new Promise((r) => setImmediate(r));
  p.run('nativeConditional("native after all")');
  assert.equal(await third, 'native after all');
  // The page aborts: both stop, and the page hears its own reason.
  p.run('var stop = new AbortController();');
  const fourth = p.run(`navigator.credentials.get({ publicKey: { challenge: new Uint8Array(1) }, mediation: 'conditional', signal: stop.signal })`);
  await new Promise((r) => setImmediate(r));
  p.run('stop.abort()');
  await assert.rejects(fourth, (e) => e.name === 'AbortError');
  assert.equal(p.sent[p.sent.length - 1].t, 'abort');
});
