// The checks webauthn.html runs on webauthn.js in a real browser, with a stand-in relay that
// answers its requests with canned replies, as SpatiMesh's isolated world or the extension's
// content script would. Results go into #results and to the test's server.
(async function () {
  'use strict';
  var params = new URLSearchParams(location.search);
  var polyfill = window.testMode === 'polyfill';
  var results = [];
  var sent = [];
  var answer = null;

  // ---- the stand-in relay ----
  document.addEventListener('spatikey-request', function (ev) {
    var request = JSON.parse(ev.detail);
    sent.push(request);
    var reply = answer ? answer(request) : null;
    if (reply) setTimeout(function () { say(Object.assign({ id: request.id }, reply)); }, 0);
  });
  function say(reply) {
    document.dispatchEvent(new CustomEvent('spatikey-reply', { detail: JSON.stringify(reply) }));
  }
  function last() {
    return sent[sent.length - 1];
  }
  function tick(ms) {
    return new Promise(function (resolve) { setTimeout(resolve, ms || 0); });
  }

  // ---- recording ----
  function check(name, ok, detail) {
    results.push({ name: name, ok: !!ok, detail: ok ? '' : String(detail) });
  }
  function canon(value) {
    if (Array.isArray(value)) return '[' + value.map(canon).join(',') + ']';
    if (value && typeof value === 'object') return '{' + Object.keys(value).sort().map(function (k) { return JSON.stringify(k) + ':' + canon(value[k]); }).join(',') + '}';
    return JSON.stringify(value);
  }
  function same(name, actual, expected) {
    var a = canon(actual), e = canon(expected);
    check(name, a === e, 'got ' + a + ' wanted ' + e);
  }
  async function rejects(name, promise, errorName) {
    try {
      var value = await promise;
      check(name, false, 'resolved with ' + String(value));
    } catch (e) {
      check(name, e && e.name === errorName, 'rejected with ' + (e && e.name) + ': ' + (e && e.message));
    }
  }
  async function step(name, fn) {
    try {
      await fn();
    } catch (e) {
      check(name, false, 'threw ' + (e && e.name) + ': ' + (e && e.message));
    }
  }

  // ---- data ----
  function b64url(bytes) {
    var text = '';
    for (var i = 0; i < bytes.length; i++) text += String.fromCharCode(bytes[i]);
    return btoa(text).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }
  function enc(text) {
    return b64url(new TextEncoder().encode(text));
  }
  function text(buffer) {
    return new TextDecoder().decode(buffer);
  }
  var CREATED = {
    credential_id: enc('credential-1'),
    client_data_json: enc('{"type":"webauthn.create"}'),
    attestation_object: enc('attestation'),
    authenticator_data: enc('authenticator-data'),
    public_key: enc('spki'),
    alg: -7,
    transports: ['internal', 'hybrid']
  };
  var ASSERTED = {
    credential_id: enc('credential-2'),
    client_data_json: enc('{"type":"webauthn.get"}'),
    authenticator_data: enc('authenticator-data-2'),
    signature: enc('signature'),
    user_handle: enc('user-1')
  };
  function createOptions(changes) {
    var options = {
      rp: { id: location.hostname, name: 'Example' },
      user: { id: new Uint8Array([1, 2, 3, 4]), name: 'paulus@example.com', displayName: 'Paulus' },
      challenge: new Uint8Array([0xfb, 0xff, 0xbf, 0x00, 0x01]).buffer,
      pubKeyCredParams: [{ type: 'public-key', alg: -7 }, { type: 'public-key', alg: -257 }],
      excludeCredentials: [{ type: 'public-key', id: new Uint8Array([9, 9]) }],
      timeout: 60000,
      extensions: { credProps: true }
    };
    return Object.assign(options, changes || {});
  }
  function getOptions(changes) {
    return Object.assign({ challenge: new Uint8Array([1, 2, 3]), rpId: location.hostname, timeout: 60000 }, changes || {});
  }
  function nativeCount() {
    return window.nativeCalls ? window.nativeCalls.length : 0;
  }

  // ---- the checks ----
  await step('which mode', async function () {
    var native = /\[native code\]/.test(Function.prototype.toString.call(PublicKeyCredential));
    check('PublicKeyCredential is ' + (polyfill ? 'webauthn.js\'s own' : 'the browser\'s'), polyfill ? !native : native, Function.prototype.toString.call(PublicKeyCredential).slice(0, 60));
    check('navigator.credentials is there', typeof navigator.credentials === 'object' && typeof navigator.credentials.create === 'function', typeof navigator.credentials);
    check('navigator.credentials is a CredentialsContainer', navigator.credentials instanceof CredentialsContainer, 'not');
  });

  await step('create', async function () {
    answer = function () { return { ok: CREATED }; };
    var credential = await navigator.credentials.create({ publicKey: createOptions() });
    var request = Object.assign({}, last());
    check('create: the request has a number for an id', typeof request.id === 'number', request.id);
    delete request.id;
    same('create: the request', request, {
      t: 'create', challenge: '-_-_AAE', rp: { name: 'Example', id: location.hostname },
      user: { id: 'AQIDBA', name: 'paulus@example.com', displayName: 'Paulus' }, algs: [-7, -257], exclude: ['CQk'], timeout: 60000
    });
    check('create: instanceof PublicKeyCredential and Credential', credential instanceof PublicKeyCredential && credential instanceof Credential, String(credential));
    check('create: the response is an AuthenticatorAttestationResponse', credential.response instanceof AuthenticatorAttestationResponse && credential.response instanceof AuthenticatorResponse, String(credential.response));
    same('create: toString', Object.prototype.toString.call(credential), '[object PublicKeyCredential]');
    same('create: id', credential.id, CREATED.credential_id);
    same('create: rawId', text(credential.rawId), 'credential-1');
    check('create: rawId is one buffer', credential.rawId === credential.rawId && credential.rawId instanceof ArrayBuffer, 'not');
    same('create: type', credential.type, 'public-key');
    same('create: authenticatorAttachment', credential.authenticatorAttachment, 'platform');
    same('create: clientDataJSON', text(credential.response.clientDataJSON), '{"type":"webauthn.create"}');
    same('create: attestationObject', text(credential.response.attestationObject), 'attestation');
    same('create: getAuthenticatorData()', text(credential.response.getAuthenticatorData()), 'authenticator-data');
    same('create: getPublicKey()', text(credential.response.getPublicKey()), 'spki');
    same('create: getPublicKeyAlgorithm()', credential.response.getPublicKeyAlgorithm(), -7);
    same('create: getTransports()', credential.response.getTransports(), ['hybrid', 'internal']);
    same('create: getClientExtensionResults()', credential.getClientExtensionResults(), { credProps: { rk: true } });
    var json = {
      id: CREATED.credential_id, rawId: CREATED.credential_id, type: 'public-key', authenticatorAttachment: 'platform',
      clientExtensionResults: { credProps: { rk: true } },
      response: {
        clientDataJSON: CREATED.client_data_json, authenticatorData: CREATED.authenticator_data, transports: ['hybrid', 'internal'],
        publicKey: CREATED.public_key, publicKeyAlgorithm: -7, attestationObject: CREATED.attestation_object
      }
    };
    same('create: toJSON()', credential.toJSON(), json);
    same('create: JSON.stringify', JSON.parse(JSON.stringify(credential)), json);
    var own = Object.getOwnPropertyNames(credential).sort();
    if (polyfill) same('create: the getters are on the prototype', own, []);
    else check('create: own getters shadow the browser\'s', ['id', 'rawId', 'response', 'toJSON', 'type'].every(function (n) { return own.indexOf(n) >= 0; }), own.join(','));
  });

  await step('get', async function () {
    answer = function () { return { ok: ASSERTED }; };
    var credential = await navigator.credentials.get({ publicKey: getOptions({ allowCredentials: [{ type: 'public-key', id: new Uint8Array([7]), transports: ['internal'] }] }) });
    var request = Object.assign({}, last());
    delete request.id;
    same('get: the request', request, { t: 'get', challenge: 'AQID', allow: ['Bw'], conditional: false, timeout: 60000, rpId: location.hostname });
    check('get: instanceof PublicKeyCredential', credential instanceof PublicKeyCredential, String(credential));
    check('get: the response is an AuthenticatorAssertionResponse', credential.response instanceof AuthenticatorAssertionResponse && credential.response instanceof AuthenticatorResponse, String(credential.response));
    same('get: signature', text(credential.response.signature), 'signature');
    same('get: authenticatorData', text(credential.response.authenticatorData), 'authenticator-data-2');
    same('get: userHandle', text(credential.response.userHandle), 'user-1');
    same('get: clientDataJSON', text(credential.response.clientDataJSON), '{"type":"webauthn.get"}');
    same('get: getClientExtensionResults()', credential.getClientExtensionResults(), {});
    same('get: toJSON()', credential.toJSON(), {
      id: ASSERTED.credential_id, rawId: ASSERTED.credential_id, type: 'public-key', authenticatorAttachment: 'platform', clientExtensionResults: {},
      response: { clientDataJSON: ASSERTED.client_data_json, authenticatorData: ASSERTED.authenticator_data, signature: ASSERTED.signature, userHandle: ASSERTED.user_handle }
    });
  });

  await step('errors', async function () {
    var cases = [['excluded', 'InvalidStateError'], ['security', 'SecurityError'], ['denied', 'NotAllowedError'], ['not_supported', 'NotSupportedError'], ['not_same_lan', 'NotAllowedError']];
    for (var i = 0; i < cases.length; i++) {
      var code = cases[i][0];
      answer = function () { return { err: { code: code, message: 'SpatiKey said no' } }; };
      await rejects('error: ' + code + ' is ' + cases[i][1], navigator.credentials.create({ publicKey: createOptions() }), cases[i][1]);
    }
    answer = function () { return { err: { code: 'no_match' } }; };
    var before = nativeCount();
    if (polyfill) {
      await rejects('error: no_match is NotAllowedError', navigator.credentials.get({ publicKey: getOptions() }), 'NotAllowedError');
    } else {
      var options = { publicKey: getOptions() };
      same('error: no_match asks the browser\'s own get()', await navigator.credentials.get(options), 'native get');
      check('error: ...with the page\'s own options and container', window.nativeCalls[before].options === options && window.nativeCalls[before].self === navigator.credentials, 'not the same');
      answer = function () { return { err: { code: 'browser' } }; };
      same('error: browser asks the browser\'s own create()', await navigator.credentials.create({ publicKey: createOptions() }), 'native create');
    }
    if (polyfill) {
      await rejects('error: no publicKey is NotSupportedError', navigator.credentials.create({}), 'NotSupportedError');
      await rejects('error: a missing member is TypeError', navigator.credentials.create({ publicKey: {} }), 'TypeError');
      await rejects('error: no ES256 is NotSupportedError', navigator.credentials.create({ publicKey: createOptions({ pubKeyCredParams: [{ type: 'public-key', alg: -257 }] }) }), 'NotSupportedError');
      await rejects('error: store() is NotSupportedError', navigator.credentials.store(null), 'NotSupportedError');
    } else {
      before = nativeCount();
      await navigator.credentials.create({ publicKey: {} });
      await navigator.credentials.create({ publicKey: createOptions({ pubKeyCredParams: [{ type: 'public-key', alg: -257 }] }) });
      await navigator.credentials.get({ password: true });
      same('error: what SpatiKey cannot do goes to the browser\'s own', nativeCount() - before, 3);
    }
    var thrown = null;
    try {
      new PublicKeyCredential();
    } catch (e) {
      thrown = e;
    }
    check('new PublicKeyCredential() is a TypeError', thrown instanceof TypeError, String(thrown));
  });

  await step('abort', async function () {
    answer = null;
    var controller = new AbortController();
    var count = sent.length;
    var asked = navigator.credentials.get({ publicKey: getOptions(), signal: controller.signal });
    await tick();
    controller.abort();
    await rejects('abort: during, AbortError', asked, 'AbortError');
    same('abort: the relay is told', sent.slice(count).map(function (r) { return r.t; }), ['get', 'abort']);
    same('abort: of the same request', sent[count + 1].id, sent[count].id);
    if (polyfill) {
      var done = new AbortController();
      done.abort();
      await rejects('abort: before, AbortError', navigator.credentials.create({ publicKey: createOptions(), signal: done.signal }), 'AbortError');
      same('abort: before, nothing is asked', sent.length, count + 2);
    }
    var mine = new AbortController();
    var again = navigator.credentials.create({ publicKey: createOptions(), signal: mine.signal });
    await tick();
    mine.abort(new RangeError('the page\'s own reason'));
    await rejects('abort: the page\'s reason is kept', again, 'RangeError');
  });

  await step('timeout', async function () {
    answer = null;
    var count = sent.length;
    await rejects('timeout: NotAllowedError', navigator.credentials.create({ publicKey: createOptions({ timeout: 30000 }) }), 'NotAllowedError');
    same('timeout: the relay is told', sent.slice(count).map(function (r) { return r.t; }), ['create', 'abort']);
  });

  await step('conditional', async function () {
    answer = null;
    var count = sent.length, before = nativeCount();
    var asked = navigator.credentials.get({ publicKey: getOptions(), mediation: 'conditional' });
    await tick(80);
    var request = sent[count];
    check('conditional: asked with conditional, no time limit', request && request.conditional === true && request.timeout === 0, JSON.stringify(request));
    check('conditional: still waiting past the timeout', sent.length === count + 1, sent.length - count);
    if (!polyfill) check('conditional: the browser\'s own waits too', nativeCount() === before + 1, nativeCount() - before);
    say({ id: request.id, ok: ASSERTED });
    var credential = await asked;
    same('conditional: SpatiKey\'s answer', credential.id, ASSERTED.credential_id);
    if (!polyfill) check('conditional: the browser\'s own is stopped', window.nativeCalls[before].options.signal.aborted, 'still waiting');
  });

  await step('statics', async function () {
    same('isUserVerifyingPlatformAuthenticatorAvailable()', await PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable(), true);
    same('isConditionalMediationAvailable()', await PublicKeyCredential.isConditionalMediationAvailable(), true);
    var capabilities = await PublicKeyCredential.getClientCapabilities();
    check('getClientCapabilities(): a platform authenticator', capabilities.passkeyPlatformAuthenticator === true && capabilities.userVerifyingPlatformAuthenticator === true, JSON.stringify(capabilities));
    var parsed = PublicKeyCredential.parseCreationOptionsFromJSON({
      rp: { name: 'Example' }, user: { id: 'AQID', name: 'a', displayName: 'A' }, challenge: '-_-_', pubKeyCredParams: [{ type: 'public-key', alg: -7 }],
      excludeCredentials: [{ type: 'public-key', id: 'Bw' }]
    });
    answer = function () { return { ok: CREATED }; };
    await navigator.credentials.create({ publicKey: parsed });
    same('parseCreationOptionsFromJSON() goes straight into create()', [last().challenge, last().user.id, last().exclude], ['-_-_', 'AQID', ['Bw']]);
    var request = PublicKeyCredential.parseRequestOptionsFromJSON({ challenge: 'AQ', allowCredentials: [{ type: 'public-key', id: 'Bw' }] });
    answer = function () { return { ok: ASSERTED }; };
    await navigator.credentials.get({ publicKey: request });
    same('parseRequestOptionsFromJSON() goes straight into get()', [last().challenge, last().allow], ['AQ', ['Bw']]);
    var bad = null;
    try {
      PublicKeyCredential.parseRequestOptionsFromJSON({ challenge: 'not+base64url' });
    } catch (e) {
      bad = e;
    }
    check('parseRequestOptionsFromJSON(): bad base64url is EncodingError', bad && bad.name === 'EncodingError', String(bad));
  });

  await step('the page\'s mischief', async function () {
    answer = function () { return { ok: CREATED }; };
    Array.prototype.toJSON = function () { return 'broken'; };
    try {
      await navigator.credentials.create({ publicKey: createOptions() });
    } finally {
      delete Array.prototype.toJSON;
    }
    same('a library\'s Array.prototype.toJSON does not reach the request', [last().algs, last().exclude], [[-7, -257], ['CQk']]);
    var frame = document.createElement('iframe');
    document.body.appendChild(frame);
    await navigator.credentials.create({ publicKey: createOptions({ challenge: new frame.contentWindow.Uint8Array([1, 2, 3]) }) });
    same('a buffer from another frame is a buffer', last().challenge, 'AQID');
    frame.remove();
    var forged = null;
    document.addEventListener('spatikey-request', function once(ev) {
      document.removeEventListener('spatikey-request', once);
      forged = JSON.parse(ev.detail);
    });
    await navigator.credentials.create({ publicKey: createOptions() });
    check('the request never carries an origin', forged && !('origin' in forged) && !('clientDataJSON' in forged), JSON.stringify(forged));
  });

  document.getElementById('results').textContent = JSON.stringify(results);
  await fetch('/results?' + params.get('run'), { method: 'POST', body: JSON.stringify({ mode: window.testMode, results: results }) });
})();
