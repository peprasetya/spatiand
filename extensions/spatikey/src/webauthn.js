// SpatiKey's WebAuthn in the page's own world: navigator.credentials.create() and get() for
// passkeys, answered by SpatiKey (docs/spatikey.md). Put into every frame at the start of every
// document, before the page's own scripts: by SpatiMesh, whose engine has no WebAuthn at all, and
// by the SpatiKey extension in Chrome and Firefox, whose own it wraps.
//
// It speaks only through events on the document, so that it needs nothing from the world it runs
// in: "spatikey-request", whose detail is a JSON string {id, t, ...}, answered by "spatikey-reply"
// with {id, ok} or {id, err: {code, message}}. A string rather than an object, because an object
// does not cross from one world to another intact in every engine. Whoever answers -- SpatiMesh's
// isolated world (cred-world.js), the extension's content script -- stamps the request with the
// frame's origin as the engine or the browser knows it, and builds clientDataJSON from that. This
// script never says what the origin is, so neither can the page, which shares its world; what it
// does say (the challenge, the relying party, the account) is the page's to say anyway.
//
// Requests: {t:"create", challenge, rp:{id?, name}, user:{id, name, displayName}, algs, exclude,
// timeout}, {t:"get", challenge, rpId?, allow, conditional, timeout} (timeout 0: none, for a
// conditional get), and {t:"abort"} with the id of the request to stop. Buffers go as base64url.
// Answers: a create's {credential_id, client_data_json, attestation_object, authenticator_data,
// public_key, alg, transports}, a get's {credential_id, client_data_json, authenticator_data,
// signature, user_handle}.
//
// With no PublicKeyCredential (SpatiMesh on WPE, where ENABLE_WEB_AUTHN is off) it defines WebAuthn
// itself -- Credential, PublicKeyCredential, the authenticator responses and a CredentialsContainer
// as navigator.credentials -- in secure contexts only, as browsers do. Where the browser has its
// own (the extensions) it keeps it and wraps create() and get(): what SpatiKey answers is handed to
// the page as objects of the browser's own classes (instanceof holds), and the browser's own is
// asked instead when SpatiKey has nothing for the site, when the person chose it, or when the
// request wants what SpatiKey cannot give (a security key, an algorithm other than ES256, PRF).
(function () {
  'use strict';
  var G = typeof window === 'object' && window ? window : globalThis;
  // Node's tests read the parts that need no page (test/webauthn.test.js).
  var testing = typeof module === 'object' && module && typeof module.exports === 'object' ? module.exports : null;

  // ---- what the page could change later, kept as it is now ----
  var O = Object, defineProperty = O.defineProperty, create = O.create, keys = O.keys, getOwn = O.getOwnPropertyDescriptor;
  var setPrototypeOf = O.setPrototypeOf, isArray = Array.isArray, arrayFrom = Array.from;
  var iterator = typeof Symbol === 'function' ? Symbol.iterator : null;
  var apply = Reflect.apply, JSONparse = JSON.parse, JSONstringify = JSON.stringify;
  var Promise_ = Promise, resolved = Promise.resolve.bind(Promise), then = Promise.prototype.then;
  var TypeError_ = TypeError, Uint8Array_ = Uint8Array, Int8Array_ = Int8Array, String_ = String, Number_ = Number;
  var isView = ArrayBuffer.isView, bufferLength = getOwn(ArrayBuffer.prototype, 'byteLength').get;
  var sliceBytes = Uint8Array.prototype.slice, sliceArray = Array.prototype.slice, join = Array.prototype.join;
  var sortArray = Array.prototype.sort, indexOf = Array.prototype.indexOf, charCodeAt = String.prototype.charCodeAt;
  var slots = new WeakMap(), slotGet = WeakMap.prototype.get, slotSet = WeakMap.prototype.set;
  var toStringTag = typeof Symbol === 'function' ? Symbol.toStringTag : null;
  var setTimeout_ = G.setTimeout, clearTimeout_ = G.clearTimeout;
  var DOMException_ = G.DOMException, CustomEvent_ = G.CustomEvent, AbortController_ = G.AbortController;
  var Target = G.EventTarget && G.EventTarget.prototype;
  var addListener = Target && Target.addEventListener, removeListener = Target && Target.removeEventListener;
  var dispatch = Target && Target.dispatchEvent;
  var doc = G.document;

  // ---- base64url, both ways ----
  var ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
  var VALUE = new Int8Array_(128);
  for (var c = 0; c < 128; c++) VALUE[c] = -1;
  for (c = 0; c < 64; c++) VALUE[apply(charCodeAt, ALPHABET, [c])] = c;

  // Bytes (a Uint8Array) as base64url without padding, as WebAuthn's JSON and SpatiKey write it.
  function toBase64url(bytes) {
    var out = '', n = bytes.length, i = 0, x;
    for (; i + 2 < n; i += 3) {
      x = bytes[i] << 16 | bytes[i + 1] << 8 | bytes[i + 2];
      out += ALPHABET[x >> 18 & 63] + ALPHABET[x >> 12 & 63] + ALPHABET[x >> 6 & 63] + ALPHABET[x & 63];
    }
    if (n - i === 1) {
      x = bytes[i] << 16;
      out += ALPHABET[x >> 18 & 63] + ALPHABET[x >> 12 & 63];
    } else if (n - i === 2) {
      x = bytes[i] << 16 | bytes[i + 1] << 8;
      out += ALPHABET[x >> 18 & 63] + ALPHABET[x >> 12 & 63] + ALPHABET[x >> 6 & 63];
    }
    return out;
  }

  // base64url, with its padding or without, as a Uint8Array; null for anything else -- standard
  // base64's "+" and "/" included, which neither WebAuthn's JSON nor SpatiKey ever writes.
  function fromBase64url(text) {
    if (typeof text !== 'string') return null;
    var end = text.length;
    while (end > 0 && apply(charCodeAt, text, [end - 1]) === 61) end--;
    if (end < text.length && (text.length % 4 !== 0 || text.length - end > 2)) return null;
    if (end % 4 === 1) return null;
    var out = new Uint8Array_(end * 3 >> 2), o = 0, acc = 0, bits = 0;
    for (var i = 0; i < end; i++) {
      var code = apply(charCodeAt, text, [i]), d = code < 128 ? VALUE[code] : -1;
      if (d < 0) return null;
      acc = (acc << 6 | d) & 0xffffff;
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        out[o++] = acc >> bits & 255;
      }
    }
    return out;
  }

  // A copy of a BufferSource's bytes, taken now as WebIDL says; null when it is not one. Not by
  // `instanceof`: a buffer made in another frame is a buffer all the same.
  function bytesOf(source) {
    if (isView(source)) return apply(sliceBytes, new Uint8Array_(source.buffer, source.byteOffset, source.byteLength), []);
    try {
      apply(bufferLength, source, []);
    } catch (e) {
      return null;
    }
    return apply(sliceBytes, new Uint8Array_(source), []);
  }

  function b64(buffer) {
    return toBase64url(new Uint8Array_(buffer));
  }

  // JSON of what this script builds itself (strings, numbers, booleans, arrays and plain objects),
  // written out by hand: a page whose library gave Array.prototype a toJSON (Prototype.js does)
  // would otherwise have every array sent as a string.
  function json(value) {
    if (value === null || value === undefined) return 'null';
    var type = typeof value;
    if (type === 'string') return JSONstringify(value);
    if (type === 'number') return isFinite(value) ? '' + value : 'null';
    if (type === 'boolean') return value ? 'true' : 'false';
    var parts = [], i;
    if (isArray(value)) {
      for (i = 0; i < value.length; i++) parts[i] = json(value[i]);
      return '[' + apply(join, parts, [',']) + ']';
    }
    var names = keys(value);
    for (i = 0; i < names.length; i++) {
      if (value[names[i]] !== undefined) parts[parts.length] = JSONstringify(names[i]) + ':' + json(value[names[i]]);
    }
    return '{' + apply(join, parts, [',']) + '}';
  }

  function copy(value) {
    return JSONparse(json(value));
  }

  // ---- what goes wrong, as browsers say it ----
  var MESSAGES = {
    NotAllowedError: 'The operation either timed out or was not allowed. See: https://www.w3.org/TR/webauthn-2/#sctn-privacy-considerations-client.',
    InvalidStateError: 'The user attempted to register an authenticator that contains one of the credentials already registered with the relying party.',
    SecurityError: 'The relying party ID is not a registrable domain suffix of, nor equal to the current domain.',
    NotSupportedError: 'The requested operation is not supported.',
    AbortError: 'The operation was aborted.'
  };

  function failure(name, message) {
    return new DOMException_(message || MESSAGES[name] || name, name);
  }

  // The page's error for a refusal. Every reason that is about the person or their devices (they
  // said no, no Mac is on this network, the Mac did not answer, there is no vault) is the same
  // NotAllowedError, as browsers make it, so that a page learns nothing from which it was.
  function errorFor(err) {
    var code = err && typeof err.code === 'string' ? err.code : '';
    switch (code) {
      case 'excluded':
      case 'invalid_state':
        return failure('InvalidStateError');
      case 'security':
      case 'security_error':
      case 'bad_rp_id':
        return failure('SecurityError');
      case 'not_supported':
      case 'unsupported':
        return failure('NotSupportedError');
      case 'aborted':
      case 'abort':
        return failure('AbortError');
      default:
        return failure('NotAllowedError');
    }
  }

  // Replies that send a wrapped call to the browser's own WebAuthn: SpatiKey has nothing for the
  // site, the person chose the browser's, or SpatiKey cannot be asked here.
  var FALLBACK = { browser: true, no_match: true };

  function fallsBack(reply) {
    return !!(reply && reply.err && FALLBACK[reply.err.code] === true);
  }

  function abortReason(signal) {
    return signal.reason !== undefined ? signal.reason : failure('AbortError');
  }

  // ---- what the page asked, as a request ----
  function required(dict, name, where) {
    if (dict[name] === undefined) {
      throw new TypeError_("Failed to read the '" + name + "' property from '" + where + "': Required member is undefined.");
    }
    return dict[name];
  }

  function dictionary(value, where) {
    if (value === undefined || value === null) return {};
    if (typeof value !== 'object' && typeof value !== 'function') {
      throw new TypeError_("The provided value is not of type '" + where + "'.");
    }
    return value;
  }

  function bufferArg(value, what) {
    var bytes = bytesOf(value);
    if (!bytes) throw new TypeError_("The provided value for '" + what + "' is not of type '(ArrayBuffer or ArrayBufferView)'.");
    return bytes;
  }

  function sequence(value, what) {
    if (value === undefined) return [];
    if (value !== null && typeof value === 'object') {
      if (isArray(value)) return apply(sliceArray, value, []);
      if (iterator && typeof value[iterator] === 'function') return arrayFrom(value);
    }
    throw new TypeError_("The provided value for '" + what + "' cannot be converted to a sequence.");
  }

  // Credential descriptors as base64url ids: only "public-key" ones, as WebAuthn ignores others.
  function descriptors(list, what) {
    var out = [], items = sequence(list, what);
    for (var i = 0; i < items.length; i++) {
      var d = dictionary(items[i], 'PublicKeyCredentialDescriptor');
      var type = String_(required(d, 'type', 'PublicKeyCredentialDescriptor'));
      var id = bufferArg(required(d, 'id', 'PublicKeyCredentialDescriptor'), what + '.id');
      if (type === 'public-key') out[out.length] = toBase64url(id);
    }
    return out;
  }

  // How long SpatiKey is waited for. A touch may be wanted on a Mac across the room, so never less
  // than half a minute; never more than the ten minutes browsers allow.
  function timeoutOf(value) {
    if (value === undefined) return 300000;
    var ms = Number_(value);
    if (!(ms >= 30000)) return 30000;
    return ms > 600000 ? 600000 : Math.floor(ms);
  }

  // navigator.credentials.create()'s publicKey as a request; TypeError for what WebIDL refuses.
  function createRequest(publicKey) {
    var where = 'PublicKeyCredentialCreationOptions';
    var options = dictionary(publicKey, where);
    var rp = dictionary(required(options, 'rp', where), 'PublicKeyCredentialRpEntity');
    var user = dictionary(required(options, 'user', where), 'PublicKeyCredentialUserEntity');
    var challenge = bufferArg(required(options, 'challenge', where), 'challenge');
    var params = sequence(required(options, 'pubKeyCredParams', where), 'pubKeyCredParams');
    var name = String_(required(rp, 'name', 'PublicKeyCredentialRpEntity'));
    var userId = bufferArg(required(user, 'id', 'PublicKeyCredentialUserEntity'), 'user.id');
    var userName = String_(required(user, 'name', 'PublicKeyCredentialUserEntity'));
    var display = String_(required(user, 'displayName', 'PublicKeyCredentialUserEntity'));
    if (userId.length < 1 || userId.length > 64) throw new TypeError_("The 'user.id' field must be between 1 and 64 bytes long.");
    var algs = [];
    for (var i = 0; i < params.length; i++) {
      var p = dictionary(params[i], 'PublicKeyCredentialParameters');
      var type = String_(required(p, 'type', 'PublicKeyCredentialParameters'));
      var alg = Number_(required(p, 'alg', 'PublicKeyCredentialParameters')) | 0;
      if (type === 'public-key') algs[algs.length] = alg;
    }
    // An empty list means "ES256 or RS256", WebAuthn says.
    if (params.length === 0) algs = [-7, -257];
    var request = {
      t: 'create',
      challenge: toBase64url(challenge),
      rp: { name: name },
      user: { id: toBase64url(userId), name: userName, displayName: display },
      algs: algs,
      exclude: descriptors(options.excludeCredentials, 'excludeCredentials'),
      timeout: timeoutOf(options.timeout)
    };
    if (rp.id !== undefined) request.rp.id = String_(rp.id);
    return request;
  }

  // navigator.credentials.get()'s publicKey as a request.
  function getRequest(publicKey, mediation) {
    var where = 'PublicKeyCredentialRequestOptions';
    var options = dictionary(publicKey, where);
    var challenge = bufferArg(required(options, 'challenge', where), 'challenge');
    var conditional = mediation === 'conditional';
    var request = {
      t: 'get',
      challenge: toBase64url(challenge),
      allow: descriptors(options.allowCredentials, 'allowCredentials'),
      conditional: conditional,
      timeout: conditional ? 0 : timeoutOf(options.timeout)
    };
    if (options.rpId !== undefined) request.rpId = String_(options.rpId);
    return request;
  }

  function first(list) {
    return isArray(list) && list.length ? String_(list[0]) : '';
  }

  // Why SpatiKey cannot make this passkey, or '' when it can: it makes ES256 keys only, it is not
  // a security key, and it has no PRF or large blobs. `strict` (polyfill mode, where nobody else
  // can be asked) refuses only what the site requires; hints are only hints there.
  function cannotMake(publicKey, request, strict) {
    if (apply(indexOf, request.algs, [-7]) < 0) return 'alg';
    var selection = publicKey.authenticatorSelection;
    if (selection && selection.authenticatorAttachment === 'cross-platform') return 'attachment';
    var ext = publicKey.extensions || {};
    if (ext.largeBlob && ext.largeBlob.support === 'required') return 'largeBlob';
    if (strict) return '';
    var hint = first(publicKey.hints);
    if (hint === 'security-key' || hint === 'hybrid') return 'hint';
    if (ext.prf !== undefined || ext.largeBlob !== undefined) return 'extension';
    if (publicKey.attestation === 'enterprise') return 'attestation';
    return '';
  }

  // Whether a wrapped get() is for SpatiKey to try at all: not when the site asks for a security
  // key first, wants PRF or large blobs, or names only credentials that live on a security key.
  function canSign(publicKey) {
    var hint = first(publicKey.hints);
    if (hint === 'security-key' || hint === 'hybrid') return false;
    var ext = publicKey.extensions || {};
    if (ext.prf !== undefined || ext.largeBlob !== undefined) return false;
    var allow = isArray(publicKey.allowCredentials) ? publicKey.allowCredentials : [];
    if (!allow.length) return true;
    for (var i = 0; i < allow.length; i++) {
      var t = allow[i] && allow[i].transports;
      if (!isArray(t) || !t.length || apply(indexOf, t, ['internal']) >= 0 || apply(indexOf, t, ['hybrid']) >= 0) return true;
    }
    return false;
  }

  // ---- asking ----
  var pending = create(null);
  var nextId = 1;

  function send(message) {
    apply(dispatch, doc, [new CustomEvent_('spatikey-request', { detail: json(message) })]);
  }

  // Ask the relay and settle with its reply, {ok} or {err}. Rejected with the signal's reason when
  // it aborts, or NotAllowedError when `ms` runs out (0: no limit); the relay is told to stop.
  function ask(message, signal, ms) {
    return new Promise_(function (resolve, reject) {
      if (signal && signal.aborted) {
        reject(abortReason(signal));
        return;
      }
      var id = nextId++, timer = 0;
      message.id = id;
      function done() {
        delete pending[id];
        if (timer) clearTimeout_(timer);
        if (signal) apply(removeListener, signal, ['abort', onAbort]);
      }
      function stop(error) {
        done();
        send({ id: id, t: 'abort' });
        reject(error);
      }
      function onAbort() {
        stop(abortReason(signal));
      }
      pending[id] = function (reply) {
        done();
        resolve(reply);
      };
      if (signal) apply(addListener, signal, ['abort', onAbort]);
      if (ms) timer = setTimeout_(function () { stop(failure('NotAllowedError')); }, ms);
      send(message);
    });
  }

  function listen() {
    apply(addListener, doc, ['spatikey-reply', function (ev) {
      var detail = ev.detail, reply;
      if (typeof detail !== 'string') return;
      try {
        reply = JSONparse(detail);
      } catch (e) {
        return;
      }
      if (!reply || typeof reply !== 'object') return;
      var waiter = pending[reply.id];
      if (typeof waiter === 'function') waiter(reply);
    }, true]);
  }

  // ---- what the page is given ----
  // The prototypes results are made from: the browser's own in wrapper mode, this script's in
  // polyfill mode. In wrapper mode the browser's getters refuse an object it did not make, so
  // each result carries its own, shadowing them.
  var classes = null;
  var wrapping = false;

  function slotOf(object, kind) {
    var slot = object !== null && (typeof object === 'object' || typeof object === 'function') ? apply(slotGet, slots, [object]) : undefined;
    if (!slot || (kind && slot.kind !== kind && !(kind === 'response' && slot.response))) throw new TypeError_('Illegal invocation');
    return slot;
  }

  var GETTERS = {
    id: function () { return slotOf(this, 'credential').id; },
    type: function () { return slotOf(this, 'credential').type; },
    rawId: function () { return slotOf(this, 'credential').rawId; },
    response: function () { return slotOf(this, 'credential').response; },
    authenticatorAttachment: function () { return slotOf(this, 'credential').attachment; },
    clientDataJSON: function () { return slotOf(this, 'response').clientDataJSON; },
    attestationObject: function () { return slotOf(this, 'attestation').attestationObject; },
    authenticatorData: function () { return slotOf(this, 'assertion').authenticatorData; },
    signature: function () { return slotOf(this, 'assertion').signature; },
    userHandle: function () { return slotOf(this, 'assertion').userHandle; }
  };

  var METHODS = {
    getClientExtensionResults: function getClientExtensionResults() { return copy(slotOf(this, 'credential').extensions); },
    toJSON: function toJSON() { return credentialJSON(slotOf(this, 'credential')); },
    getTransports: function getTransports() { return apply(sliceArray, slotOf(this, 'attestation').transports, []); },
    getAuthenticatorData: function getAuthenticatorData() { return slotOf(this, 'attestation').authenticatorData; },
    getPublicKey: function getPublicKey() { return slotOf(this, 'attestation').publicKey; },
    getPublicKeyAlgorithm: function getPublicKeyAlgorithm() { return slotOf(this, 'attestation').alg; }
  };

  var CREDENTIAL_OWN = ['id', 'type', 'rawId', 'response', 'authenticatorAttachment', 'getClientExtensionResults', 'toJSON'];
  var ATTESTATION_OWN = ['clientDataJSON', 'attestationObject', 'getTransports', 'getAuthenticatorData', 'getPublicKey', 'getPublicKeyAlgorithm'];
  var ASSERTION_OWN = ['clientDataJSON', 'authenticatorData', 'signature', 'userHandle'];

  function make(proto, slot, own) {
    var object = create(proto);
    apply(slotSet, slots, [object, slot]);
    if (wrapping) {
      for (var i = 0; i < own.length; i++) {
        var name = own[i];
        if (GETTERS[name]) defineProperty(object, name, { get: GETTERS[name], enumerable: true, configurable: true });
        else defineProperty(object, name, { value: METHODS[name], writable: true, configurable: true });
      }
    }
    return object;
  }

  function need(text) {
    var bytes = fromBase64url(text);
    // An answer that cannot be read is SpatiKey's fault, not the page's: the same refusal as any.
    if (!bytes) throw failure('NotAllowedError');
    return bytes;
  }

  function credential(rawId, response, extensions) {
    return make(classes.credential, {
      kind: 'credential',
      id: toBase64url(rawId),
      rawId: rawId.buffer,
      type: 'public-key',
      attachment: 'platform',
      response: response,
      extensions: extensions
    }, CREDENTIAL_OWN);
  }

  // A create's answer as a PublicKeyCredential with an AuthenticatorAttestationResponse.
  function attestationFrom(ok, extensions) {
    ok = ok || {};
    var rawId = need(ok.credential_id);
    var transports = [];
    if (isArray(ok.transports)) {
      for (var i = 0; i < ok.transports.length; i++) {
        if (typeof ok.transports[i] === 'string' && apply(indexOf, transports, [ok.transports[i]]) < 0) transports[transports.length] = ok.transports[i];
      }
      apply(sortArray, transports, []);
    } else {
      transports = ['hybrid', 'internal'];
    }
    var response = make(classes.attestation, {
      kind: 'attestation',
      response: true,
      clientDataJSON: need(ok.client_data_json).buffer,
      attestationObject: need(ok.attestation_object).buffer,
      authenticatorData: need(ok.authenticator_data).buffer,
      publicKey: ok.public_key ? need(ok.public_key).buffer : null,
      alg: typeof ok.alg === 'number' ? ok.alg : -7,
      transports: transports
    }, ATTESTATION_OWN);
    var results = {};
    // Every passkey SpatiKey makes is discoverable.
    if (extensions && extensions.credProps) results.credProps = { rk: true };
    return credential(rawId, response, results);
  }

  // A get's answer as a PublicKeyCredential with an AuthenticatorAssertionResponse.
  function assertionFrom(ok, extensions) {
    ok = ok || {};
    var rawId = need(ok.credential_id);
    var response = make(classes.assertion, {
      kind: 'assertion',
      response: true,
      clientDataJSON: need(ok.client_data_json).buffer,
      authenticatorData: need(ok.authenticator_data).buffer,
      signature: need(ok.signature).buffer,
      userHandle: ok.user_handle ? need(ok.user_handle).buffer : null
    }, ASSERTION_OWN);
    var results = {};
    if (extensions && extensions.appid !== undefined) results.appid = false;
    return credential(rawId, response, results);
  }

  // toJSON(): RegistrationResponseJSON or AuthenticationResponseJSON, as WebAuthn Level 3 has them.
  function credentialJSON(slot) {
    var r = apply(slotGet, slots, [slot.response]), response;
    if (r.kind === 'attestation') {
      response = { clientDataJSON: b64(r.clientDataJSON), authenticatorData: b64(r.authenticatorData), transports: apply(sliceArray, r.transports, []) };
      if (r.publicKey) response.publicKey = b64(r.publicKey);
      response.publicKeyAlgorithm = r.alg;
      response.attestationObject = b64(r.attestationObject);
    } else {
      response = { clientDataJSON: b64(r.clientDataJSON), authenticatorData: b64(r.authenticatorData), signature: b64(r.signature) };
      if (r.userHandle) response.userHandle = b64(r.userHandle);
    }
    return {
      id: slot.id,
      rawId: b64(slot.rawId),
      response: response,
      authenticatorAttachment: slot.attachment,
      clientExtensionResults: copy(slot.extensions),
      type: slot.type
    };
  }

  // ---- parseCreationOptionsFromJSON, parseRequestOptionsFromJSON ----
  function shallow(value) {
    var out = {}, names = keys(value);
    for (var i = 0; i < names.length; i++) out[names[i]] = value[names[i]];
    return out;
  }

  function decoded(value, what) {
    var bytes = fromBase64url(String_(value));
    if (!bytes) throw failure('EncodingError', "The '" + what + "' field is not base64url.");
    return bytes.buffer;
  }

  function descriptorsFromJSON(list, what) {
    var items = sequence(list, what), out = [];
    for (var i = 0; i < items.length; i++) {
      var d = shallow(dictionary(items[i], 'PublicKeyCredentialDescriptorJSON'));
      required(d, 'type', 'PublicKeyCredentialDescriptorJSON');
      d.id = decoded(required(d, 'id', 'PublicKeyCredentialDescriptorJSON'), what + '.id');
      out[i] = d;
    }
    return out;
  }

  function parseCreation(json) {
    var where = 'PublicKeyCredentialCreationOptionsJSON';
    var options = dictionary(json, where);
    var out = shallow(options);
    required(options, 'rp', where);
    required(options, 'pubKeyCredParams', where);
    var user = shallow(dictionary(required(options, 'user', where), 'PublicKeyCredentialUserEntityJSON'));
    user.id = decoded(required(user, 'id', 'PublicKeyCredentialUserEntityJSON'), 'user.id');
    out.user = user;
    out.challenge = decoded(required(options, 'challenge', where), 'challenge');
    if (options.excludeCredentials !== undefined) out.excludeCredentials = descriptorsFromJSON(options.excludeCredentials, 'excludeCredentials');
    return out;
  }

  function parseRequest(json) {
    var where = 'PublicKeyCredentialRequestOptionsJSON';
    var options = dictionary(json, where);
    var out = shallow(options);
    out.challenge = decoded(required(options, 'challenge', where), 'challenge');
    if (options.allowCredentials !== undefined) out.allowCredentials = descriptorsFromJSON(options.allowCredentials, 'allowCredentials');
    return out;
  }

  function capabilities() {
    return {
      conditionalCreate: false,
      conditionalGet: true,
      hybridTransport: false,
      passkeyPlatformAuthenticator: true,
      relatedOrigins: false,
      signalAllAcceptedCredentials: false,
      signalCurrentUserDetails: false,
      signalUnknownCredential: false,
      userVerifyingPlatformAuthenticator: true,
      'extension:credProps': true
    };
  }

  function signalOf(options) {
    var signal = options.signal;
    if (signal === undefined || signal === null) return null;
    if (typeof signal !== 'object' || typeof signal.aborted !== 'boolean' || typeof signal.addEventListener !== 'function') {
      throw new TypeError_("Failed to read the 'signal' property: The provided value is not of type 'AbortSignal'.");
    }
    return signal;
  }

  // ---- polyfill mode: WebAuthn where the engine has none ----
  function polyCreate(options) {
    return new Promise_(function (resolve) {
      var o = dictionary(options, 'CredentialCreationOptions');
      var signal = signalOf(o);
      if (o.publicKey === undefined) throw failure('NotSupportedError', 'Only public key credentials can be created here.');
      var request = createRequest(o.publicKey);
      if (signal && signal.aborted) throw abortReason(signal);
      if (o.mediation === 'conditional') throw failure('NotSupportedError', 'Conditional creation is not supported.');
      var why = cannotMake(o.publicKey, request, true);
      if (why === 'alg') throw failure('NotSupportedError', 'None of the requested algorithms is supported: SpatiKey makes ES256 keys.');
      if (why) throw failure('NotAllowedError');
      var extensions = o.publicKey.extensions;
      resolve(apply(then, ask(request, signal, request.timeout), [function (reply) {
        if (reply.err) throw errorFor(reply.err);
        return attestationFrom(reply.ok, extensions);
      }]));
    });
  }

  function polyGet(options) {
    return new Promise_(function (resolve) {
      var o = dictionary(options, 'CredentialRequestOptions');
      var signal = signalOf(o);
      if (o.publicKey === undefined) throw failure('NotSupportedError', 'Only public key credentials can be asked for here.');
      var request = getRequest(o.publicKey, o.mediation);
      if (signal && signal.aborted) throw abortReason(signal);
      // Nothing is given without the person's say: a silent request finds nothing.
      if (o.mediation === 'silent') {
        resolve(null);
        return;
      }
      var extensions = o.publicKey.extensions;
      resolve(apply(then, ask(request, signal, request.timeout), [function (reply) {
        if (reply.err) throw errorFor(reply.err);
        return assertionFrom(reply.ok, extensions);
      }]));
    });
  }

  function polyStore() {
    return Promise_.reject(failure('NotSupportedError', 'Storing credentials is not supported.'));
  }

  function polyPreventSilentAccess() {
    return resolved(undefined);
  }

  function illegal() {
    throw new TypeError_('Illegal constructor');
  }

  function method(target, name, fn, length) {
    defineProperty(fn, 'length', { value: length, configurable: true });
    var d = getOwn(target, name);
    defineProperty(target, name, { value: fn, writable: true, enumerable: d ? d.enumerable : true, configurable: true });
  }

  // `ctor`'s prototype, inheriting `parent`'s, with getters and methods as WebIDL lays them out.
  function setup(ctor, parent, tag, getters, methods) {
    var proto = create(parent ? parent.prototype : O.prototype);
    defineProperty(proto, 'constructor', { value: ctor, writable: true, configurable: true });
    for (var i = 0; i < getters.length; i++) defineProperty(proto, getters[i], { get: GETTERS[getters[i]], enumerable: true, configurable: true });
    var names = keys(methods);
    for (i = 0; i < names.length; i++) defineProperty(proto, names[i], { value: methods[names[i]], writable: true, enumerable: true, configurable: true });
    if (toStringTag) defineProperty(proto, toStringTag, { value: tag, configurable: true });
    defineProperty(ctor, 'prototype', { value: proto, writable: false });
    if (parent) setPrototypeOf(ctor, parent);
    return proto;
  }

  function global(name, value) {
    defineProperty(G, name, { value: value, writable: true, enumerable: false, configurable: true });
  }

  function polyfill(nav) {
    function Credential() { illegal(); }
    function PublicKeyCredential() { illegal(); }
    function AuthenticatorResponse() { illegal(); }
    function AuthenticatorAttestationResponse() { illegal(); }
    function AuthenticatorAssertionResponse() { illegal(); }
    // An engine with Credential Management but no WebAuthn keeps its Credential: ours are made
    // from it, with their own id and type.
    var Base = typeof G.Credential === 'function' ? G.Credential : null;
    if (!Base) {
      setup(Credential, null, 'Credential', ['id', 'type'], {});
      method(Credential, 'isConditionalMediationAvailable', function isConditionalMediationAvailable() { return resolved(false); }, 0);
      global('Credential', Credential);
      Base = Credential;
    }
    var own = Base === Credential ? [] : ['id', 'type'];
    setup(PublicKeyCredential, Base, 'PublicKeyCredential', own.concat(['rawId', 'response', 'authenticatorAttachment']), {
      getClientExtensionResults: METHODS.getClientExtensionResults,
      toJSON: METHODS.toJSON
    });
    method(PublicKeyCredential, 'isUserVerifyingPlatformAuthenticatorAvailable', function isUserVerifyingPlatformAuthenticatorAvailable() { return resolved(true); }, 0);
    method(PublicKeyCredential, 'isConditionalMediationAvailable', function isConditionalMediationAvailable() { return resolved(true); }, 0);
    method(PublicKeyCredential, 'getClientCapabilities', function getClientCapabilities() { return resolved(capabilities()); }, 0);
    method(PublicKeyCredential, 'parseCreationOptionsFromJSON', function parseCreationOptionsFromJSON(options) { return parseCreation(options); }, 1);
    method(PublicKeyCredential, 'parseRequestOptionsFromJSON', function parseRequestOptionsFromJSON(options) { return parseRequest(options); }, 1);
    setup(AuthenticatorResponse, null, 'AuthenticatorResponse', ['clientDataJSON'], {});
    setup(AuthenticatorAttestationResponse, AuthenticatorResponse, 'AuthenticatorAttestationResponse', ['attestationObject'], {
      getTransports: METHODS.getTransports,
      getAuthenticatorData: METHODS.getAuthenticatorData,
      getPublicKey: METHODS.getPublicKey,
      getPublicKeyAlgorithm: METHODS.getPublicKeyAlgorithm
    });
    setup(AuthenticatorAssertionResponse, AuthenticatorResponse, 'AuthenticatorAssertionResponse', ['authenticatorData', 'signature', 'userHandle'], {});
    global('PublicKeyCredential', PublicKeyCredential);
    global('AuthenticatorResponse', AuthenticatorResponse);
    global('AuthenticatorAttestationResponse', AuthenticatorAttestationResponse);
    global('AuthenticatorAssertionResponse', AuthenticatorAssertionResponse);
    classes = {
      credential: PublicKeyCredential.prototype,
      attestation: AuthenticatorAttestationResponse.prototype,
      assertion: AuthenticatorAssertionResponse.prototype
    };

    var Existing = G.CredentialsContainer;
    if (typeof Existing === 'function' && nav.credentials) {
      // Credential Management without WebAuthn: its container keeps the other kinds.
      var proto = Existing.prototype, nativeCreate = proto.create, nativeGet = proto.get;
      method(proto, 'create', function create(options) {
        return options && typeof options === 'object' && options.publicKey !== undefined ? polyCreate(options) : apply(nativeCreate, this, arguments);
      }, 0);
      method(proto, 'get', function get(options) {
        return options && typeof options === 'object' && options.publicKey !== undefined ? polyGet(options) : apply(nativeGet, this, arguments);
      }, 0);
      return;
    }
    function CredentialsContainer() { illegal(); }
    setup(CredentialsContainer, null, 'CredentialsContainer', [], {
      create: function create(options) { return polyCreate(options); },
      get: function get(options) { return polyGet(options); },
      // store(credential): a passkey is stored by making it, and there are no other kinds here.
      store: function store(credential) { return polyStore(); },
      preventSilentAccess: function preventSilentAccess() { return polyPreventSilentAccess(); }
    });
    var methods = CredentialsContainer.prototype;
    defineProperty(methods.create, 'length', { value: 0 });
    defineProperty(methods.get, 'length', { value: 0 });
    global('CredentialsContainer', CredentialsContainer);
    var container = create(methods);
    var holder = G.Navigator && G.Navigator.prototype && nav instanceof G.Navigator ? G.Navigator.prototype : nav;
    defineProperty(holder, 'credentials', { get: function credentials() { return container; }, enumerable: true, configurable: true });
  }

  // ---- wrapper mode: the browser's own, with SpatiKey asked first ----
  function wrapCreate(nativeCreate) {
    return function create(options) {
      var self = this, args = arguments;
      if (!options || typeof options !== 'object' || options.publicKey === undefined) return apply(nativeCreate, self, args);
      var request;
      try {
        request = createRequest(options.publicKey);
        signalOf(options);
      } catch (e) {
        // Let the browser say what is wrong with it, in its own words.
        return apply(nativeCreate, self, args);
      }
      if (options.mediation === 'conditional' || (options.signal && options.signal.aborted)) return apply(nativeCreate, self, args);
      if (cannotMake(options.publicKey, request, false)) return apply(nativeCreate, self, args);
      var extensions = options.publicKey.extensions;
      return apply(then, ask(request, options.signal, request.timeout), [function (reply) {
        if (fallsBack(reply)) return apply(nativeCreate, self, args);
        if (reply.err) throw errorFor(reply.err);
        return attestationFrom(reply.ok, extensions);
      }]);
    };
  }

  function wrapGet(nativeGet) {
    return function get(options) {
      var self = this, args = arguments;
      if (!options || typeof options !== 'object' || options.publicKey === undefined || options.mediation === 'silent') return apply(nativeGet, self, args);
      var request;
      try {
        request = getRequest(options.publicKey, options.mediation);
        signalOf(options);
      } catch (e) {
        return apply(nativeGet, self, args);
      }
      if ((options.signal && options.signal.aborted) || !canSign(options.publicKey)) return apply(nativeGet, self, args);
      var extensions = options.publicKey.extensions;
      if (request.conditional) return race(self, nativeGet, options, request, extensions);
      return apply(then, ask(request, options.signal, request.timeout), [function (reply) {
        if (fallsBack(reply)) return apply(nativeGet, self, args);
        if (reply.err) throw errorFor(reply.err);
        return assertionFrom(reply.ok, extensions);
      }]);
    };
  }

  // A conditional get, in wrapper mode: the browser's own (its passkeys in the field's autofill)
  // and SpatiKey's (the key button in the field) both wait, and whichever the person uses first
  // answers the page; the other is stopped.
  function race(self, nativeGet, options, request, extensions) {
    var signal = options.signal;
    var theirs = new AbortController_(), ours = new AbortController_();
    var asked = shallow(options);
    asked.signal = theirs.signal;
    return new Promise_(function (resolve, reject) {
      var settled = false, theirFailure = null, theirsOver = false, oursOver = false;
      function win(value) {
        if (settled) return;
        settled = true;
        resolve(value);
      }
      function lose(error) {
        if (settled) return;
        settled = true;
        reject(error);
      }
      if (signal) {
        apply(addListener, signal, ['abort', function () {
          var reason = abortReason(signal);
          theirs.abort(reason);
          ours.abort(reason);
          lose(reason);
        }]);
      }
      apply(then, apply(nativeGet, self, [asked]), [function (value) {
        theirsOver = true;
        ours.abort();
        win(value);
      }, function (error) {
        theirsOver = true;
        theirFailure = error;
        // Stopped for SpatiKey's answer, or by the page: already settled.
        if (settled) return;
        if (oursOver) lose(error);
      }]);
      apply(then, ask(request, ours.signal, 0), [function (reply) {
        if (settled) return;
        if (fallsBack(reply)) {
          oursOver = true;
          if (theirsOver) lose(theirFailure || failure('NotAllowedError'));
          return;
        }
        var value;
        try {
          if (reply.err) throw errorFor(reply.err);
          value = assertionFrom(reply.ok, extensions);
        } catch (error) {
          oursOver = true;
          theirs.abort();
          lose(error);
          return;
        }
        theirs.abort();
        win(value);
      }, function (error) {
        oursOver = true;
        if (!settled && theirsOver) lose(theirFailure || error);
      }]);
    });
  }

  function wrap() {
    var Container = G.CredentialsContainer, Credential = G.PublicKeyCredential;
    if (!G.AuthenticatorAttestationResponse || !G.AuthenticatorAssertionResponse) return;
    classes = {
      credential: Credential.prototype,
      attestation: G.AuthenticatorAttestationResponse.prototype,
      assertion: G.AuthenticatorAssertionResponse.prototype
    };
    wrapping = true;
    var proto = Container.prototype;
    var nativeCreate = proto.create, nativeGet = proto.get;
    method(proto, 'create', wrapCreate(nativeCreate), nativeCreate.length);
    method(proto, 'get', wrapGet(nativeGet), nativeGet.length);
    // SpatiKey is a platform authenticator that verifies its user, wherever the browser is.
    method(Credential, 'isUserVerifyingPlatformAuthenticatorAvailable', function isUserVerifyingPlatformAuthenticatorAvailable() { return resolved(true); }, 0);
    var nativeCapabilities = Credential.getClientCapabilities;
    if (typeof nativeCapabilities === 'function') {
      method(Credential, 'getClientCapabilities', function getClientCapabilities() {
        return apply(then, apply(nativeCapabilities, Credential, []), [function (found) {
          var out = found && typeof found === 'object' ? found : {};
          out.conditionalGet = true;
          out.passkeyPlatformAuthenticator = true;
          out.userVerifyingPlatformAuthenticator = true;
          out['extension:credProps'] = true;
          return out;
        }]);
      }, 0);
    } else {
      method(Credential, 'getClientCapabilities', function getClientCapabilities() { return resolved(capabilities()); }, 0);
    }
    if (typeof Credential.parseCreationOptionsFromJSON !== 'function') {
      method(Credential, 'parseCreationOptionsFromJSON', function parseCreationOptionsFromJSON(options) { return parseCreation(options); }, 1);
    }
    if (typeof Credential.parseRequestOptionsFromJSON !== 'function') {
      method(Credential, 'parseRequestOptionsFromJSON', function parseRequestOptionsFromJSON(options) { return parseRequest(options); }, 1);
    }
  }

  function install() {
    if (!doc || !dispatch || !CustomEvent_ || !DOMException_ || !G.isSecureContext) return;
    var nav = G.navigator;
    if (!nav) return;
    if (G.__spatikeyWebAuthn) return;
    try {
      defineProperty(G, '__spatikeyWebAuthn', { value: true });
    } catch (e) {
      return;
    }
    if (typeof G.PublicKeyCredential === 'function' && typeof G.CredentialsContainer === 'function' && nav.credentials) wrap();
    else polyfill(nav);
    if (classes) listen();
  }

  if (testing) {
    testing.toBase64url = toBase64url;
    testing.fromBase64url = fromBase64url;
    testing.bytesOf = bytesOf;
    testing.json = json;
    testing.createRequest = createRequest;
    testing.getRequest = getRequest;
    testing.errorFor = errorFor;
    testing.cannotMake = cannotMake;
    testing.canSign = canSign;
    testing.timeoutOf = timeoutOf;
  }
  install();
})();
