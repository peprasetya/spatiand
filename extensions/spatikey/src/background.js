// SpatiKey's extension, its background: a service worker in Chrome, an event page in Firefox
// (docs/spatikey.md, "The extensions"). It is the only part that talks to SpatiKey, through the
// native-messaging host "id.prasetya.spatikey" (spatikey-host, which relays to the service's
// socket), in the service's own protocol: {"id", "who", "op", ...} answered by {"id", "ok"} or
// {"id", "err"} (crates/spatikey-core/src/wire.rs).
//
// The origin a request is for is always the browser's word -- the sender's origin, or the
// frame's URL -- never what a page or a content script says. From it this builds clientDataJSON
// exactly as browsers do, and gives SpatiKey only its SHA-256. Pages that are not secure are
// refused, and so are frames inside another site's page: there the browser's own WebAuthn knows
// the frame's permissions policy, and SpatiKey does not, so it is left to the browser.
'use strict';
(function () {
  var api = typeof browser === 'object' && browser && browser.runtime ? browser : chrome;
  // Node's tests read the parts that need no browser (test/background.test.js).
  var testing = typeof module === 'object' && module && typeof module.exports === 'object' ? module.exports : null;

  var HOST = 'id.prasetya.spatikey';
  // How long SpatiKey is waited for: a person may be asked on a Mac across the room first.
  var NATIVE_MS = 180000;
  // How long the helper is kept after the last answer, before it is let go.
  var IDLE_MS = 300000;
  // How long a login the person sent can still be saved.
  var OFFER_MS = 120000;

  // ---- origins ----
  // The origin of the frame a message came from, as the browser says it. When the browser says
  // "null" the document has no origin of its own -- a frame sandboxed without allow-same-origin,
  // or a page served with "Content-Security-Policy: sandbox", which is how a site shows files it
  // does not vouch for from its own address -- and that is the answer: its address names a site
  // it is not, so nothing is filled there and no passkey is used (the browser's own WebAuthn
  // refuses such a document too). The address is read only where the browser gives no origin at
  // all.
  function originOf(sender) {
    if (!sender) return null;
    if (sender.origin === 'null') return null;
    if (typeof sender.origin === 'string' && sender.origin) return sender.origin;
    try {
      var origin = new URL(sender.url).origin;
      return origin && origin !== 'null' ? origin : null;
    } catch (e) {
      return null;
    }
  }

  function hostOf(origin) {
    try {
      var host = new URL(origin).hostname.toLowerCase();
      return host.charAt(0) === '[' ? host.slice(1, -1) : host;
    } catch (e) {
      return '';
    }
  }

  function isIp(host) {
    return /^\d{1,3}(\.\d{1,3}){3}$/.test(host) || host.indexOf(':') >= 0;
  }

  function isLocal(host) {
    return host === 'localhost' || /\.localhost$/.test(host) || /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host) || host === '::1';
  }

  // https, or http to this computer: what WebAuthn and filling accept, as spatikey-core's
  // site::is_secure_origin has it.
  function isSecure(origin) {
    var url;
    try {
      url = new URL(origin);
    } catch (e) {
      return false;
    }
    if (url.protocol === 'https:') return true;
    return url.protocol === 'http:' && isLocal(hostOf(origin));
  }

  // May a page on `host` use the relying party id `rpId`? Its own host or a parent domain of it,
  // an IP address only as the host itself. A parent that is a public suffix ("co.uk") passes here;
  // the service, which has the Public Suffix List, refuses it.
  function rpIdOk(rpId, host) {
    if (typeof rpId !== 'string' || !rpId || !host) return false;
    rpId = rpId.toLowerCase();
    if (rpId === host) return true;
    if (isIp(host) || isIp(rpId)) return false;
    if (rpId === 'localhost') return isLocal(host);
    return rpId.indexOf('.') > 0 && host.slice(-(rpId.length + 1)) === '.' + rpId;
  }

  // ---- clientDataJSON, as browsers write it ----
  function clientDataJSON(type, challenge, origin, crossOrigin, topOrigin) {
    var out = '{"type":' + JSON.stringify(type) + ',"challenge":' + JSON.stringify(challenge) + ',"origin":' + JSON.stringify(origin) +
      ',"crossOrigin":' + (crossOrigin ? 'true' : 'false');
    if (crossOrigin && topOrigin) out += ',"topOrigin":' + JSON.stringify(topOrigin);
    return out + '}';
  }

  function utf8(text) {
    return new TextEncoder().encode(text);
  }

  function b64url(bytes) {
    var text = '';
    for (var i = 0; i < bytes.length; i++) text += String.fromCharCode(bytes[i]);
    return btoa(text).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }

  function sha256(text) {
    return crypto.subtle.digest('SHA-256', utf8(text)).then(function (digest) { return b64url(new Uint8Array(digest)); });
  }

  var BASE64URL = /^[A-Za-z0-9_-]*$/;

  function strings(list) {
    return Array.isArray(list) ? list.filter(function (s) { return typeof s === 'string' && BASE64URL.test(s); }) : [];
  }

  function numbers(list) {
    return Array.isArray(list) ? list.filter(function (n) { return typeof n === 'number' && isFinite(n); }) : [];
  }

  // What a page is told of SpatiKey's refusals. In a browser that has its own WebAuthn, whatever
  // keeps SpatiKey from answering (not installed, not set up, no Mac near, a request it does not
  // understand) hands the page to the browser's own; only the person's "no", an account already
  // there, nothing for this site, or the Mac being too slow are the page's to hear.
  function pageError(err) {
    var code = err && err.code;
    if (code === 'excluded' || code === 'no_match' || code === 'denied' || code === 'timeout') return { code: code, message: (err && err.message) || '' };
    return { code: 'browser', message: (err && err.message) || String(code || '') };
  }

  function browserName() {
    var nav = typeof navigator === 'object' && navigator ? navigator : {};
    var ua = nav.userAgent || '';
    if (/Firefox\//.test(ua)) return 'Firefox';
    var brands = nav.userAgentData && Array.isArray(nav.userAgentData.brands) ? nav.userAgentData.brands.map(function (b) { return b.brand; }).join(' ') : '';
    if (/Brave/.test(brands)) return 'Brave';
    if (/Microsoft Edge/.test(brands) || /Edg\//.test(ua)) return 'Edge';
    if (/Opera/.test(brands) || /OPR\//.test(ua)) return 'Opera';
    if (/Chromium/.test(brands) && !/Google Chrome/.test(brands)) return 'Chromium';
    return 'Chrome';
  }

  // ---- the helper ----
  var port = null;
  var waiting = new Map();
  var nextId = 1;
  var lastError = '';
  var idleTimer = 0;
  var holds = 0;

  function letGo() {
    idleTimer = 0;
    if (port && !waiting.size && !holds) {
      var p = port;
      port = null;
      p.disconnect();
    }
  }

  function idleSoon() {
    clearTimeout(idleTimer);
    idleTimer = setTimeout(letGo, IDLE_MS);
  }

  function connect() {
    if (port) return port;
    var p;
    try {
      p = api.runtime.connectNative(HOST);
    } catch (e) {
      lastError = String((e && e.message) || e);
      return null;
    }
    port = p;
    p.onMessage.addListener(function (message) {
      if (!message || typeof message !== 'object') return;
      var w = waiting.get(message.id);
      if (!w) return;
      waiting.delete(message.id);
      clearTimeout(w.timer);
      w.resolve(message);
      idleSoon();
    });
    p.onDisconnect.addListener(function (gone) {
      var error = (gone && gone.error && gone.error.message) || (api.runtime.lastError && api.runtime.lastError.message) || 'SpatiKey’s helper stopped.';
      lastError = error;
      if (port === p) port = null;
      var all = Array.from(waiting.values());
      waiting.clear();
      all.forEach(function (w) {
        clearTimeout(w.timer);
        w.resolve({ err: { code: 'unavailable', message: error } });
      });
    });
    return p;
  }

  // Ask SpatiKey; settles with its answer, {ok} or {err}, and never rejects.
  function native(request) {
    return new Promise(function (resolve) {
      var p = connect();
      if (!p) {
        resolve({ err: { code: 'unavailable', message: lastError || 'SpatiKey’s helper could not be started.' } });
        return;
      }
      clearTimeout(idleTimer);
      var id = nextId++;
      var envelope = { id: id, who: { app: browserName() } };
      Object.keys(request).forEach(function (k) { envelope[k] = request[k]; });
      var timer = setTimeout(function () {
        if (waiting.delete(id)) resolve({ err: { code: 'timeout', message: 'SpatiKey did not answer in time.' } });
      }, NATIVE_MS);
      waiting.set(id, { resolve: resolve, timer: timer });
      try {
        p.postMessage(envelope);
      } catch (e) {
        waiting.delete(id);
        clearTimeout(timer);
        resolve({ err: { code: 'unavailable', message: String((e && e.message) || e) } });
      }
    });
  }

  // Keep the helper while something may still be asked of it: in Chrome an open native port is
  // also what keeps this worker, and what it remembers, alive.
  function hold() {
    holds++;
    connect();
    clearTimeout(idleTimer);
  }

  function release() {
    if (holds > 0) holds--;
    if (!holds) idleSoon();
  }

  // ---- WebAuthn ----
  // Requests being answered, by tab, frame, document and the page's id: what an abort finds.
  var inflight = new Map();

  function keyOf(sender, message, id) {
    return [sender.tab.id, sender.frameId || 0, message.doc || '', id].join(':');
  }

  function frameOf(sender) {
    var f = { tab: sender.tab.id, frame: sender.frameId || 0, documentId: sender.documentId || '', aborted: false, choosing: false };
    return f;
  }

  function target(f) {
    var to = { frameId: f.frame };
    if (f.documentId) to.documentId = f.documentId;
    return to;
  }

  function busyTab(tab) {
    var busy = false;
    inflight.forEach(function (f) { if (f.tab === tab && !f.aborted) busy = true; });
    return busy;
  }

  // Ask the frame to show a chooser; settles with {choice}, {browser} or {cancel}.
  function choose(f, message) {
    message.k = 'choose';
    f.choosing = true;
    return Promise.resolve(api.tabs.sendMessage(f.tab, message, target(f))).then(function (answer) {
      f.choosing = false;
      return answer && typeof answer === 'object' ? answer : { cancel: true };
    }, function () {
      f.choosing = false;
      return { cancel: true };
    });
  }

  // Is the sending frame inside a page of another origin? The content script's own reading (it
  // can look up its frames, the page cannot fake that), and the tab's address where the browser
  // gives it.
  function crossFrame(sender, origin, said) {
    if (!sender.frameId) return false;
    if (said) return true;
    if (sender.tab && sender.tab.url) {
      try {
        return new URL(sender.tab.url).origin !== origin;
      } catch (e) {
        return true;
      }
    }
    return false;
  }

  function createPasskey(request, origin, f) {
    var host = hostOf(origin), rp = request.rp || {}, user = request.user || {};
    var rpId = typeof rp.id === 'string' ? rp.id.toLowerCase() : host;
    if (!rpIdOk(rpId, host)) return Promise.resolve({ err: { code: 'security', message: 'The relying party is not this site.' } });
    if (typeof request.challenge !== 'string' || !BASE64URL.test(request.challenge) || typeof user.id !== 'string' || !BASE64URL.test(user.id)) {
      return Promise.resolve({ err: { code: 'bad_request', message: 'A request this extension cannot read.' } });
    }
    var data = clientDataJSON('webauthn.create', request.challenge, origin, false, null);
    return sha256(data).then(function (hash) {
      return native({
        op: 'passkey_create',
        origin: origin,
        rp_id: rpId,
        rp_name: String(rp.name || ''),
        user_id: user.id,
        user_name: String(user.name || ''),
        user_display: String(user.displayName || ''),
        client_data_hash: hash,
        exclude: strings(request.exclude),
        algs: numbers(request.algs)
      });
    }).then(function (answer) {
      if (f.aborted) return { err: { code: 'aborted', message: '' } };
      if (answer.err) return { err: pageError(answer.err) };
      var ok = answer.ok || {};
      return {
        ok: {
          credential_id: ok.credential_id,
          client_data_json: b64url(utf8(data)),
          attestation_object: ok.attestation_object,
          authenticator_data: ok.authenticator_data,
          public_key: ok.public_key,
          alg: ok.alg,
          transports: ok.transports
        }
      };
    });
  }

  function getPasskey(request, origin, f) {
    var host = hostOf(origin);
    var rpId = typeof request.rpId === 'string' ? request.rpId.toLowerCase() : host;
    if (!rpIdOk(rpId, host)) return Promise.resolve({ err: { code: 'security', message: 'The relying party is not this site.' } });
    if (typeof request.challenge !== 'string' || !BASE64URL.test(request.challenge)) {
      return Promise.resolve({ err: { code: 'bad_request', message: 'A request this extension cannot read.' } });
    }
    var data = clientDataJSON('webauthn.get', request.challenge, origin, false, null);
    return sha256(data).then(function (hash) {
      return native({
        op: 'passkey_get',
        origin: origin,
        rp_id: rpId,
        client_data_hash: hash,
        allow: strings(request.allow),
        conditional: request.conditional === true
      });
    }).then(function (answer) {
      if (f.aborted) return { err: { code: 'aborted', message: '' } };
      if (answer.err) return { err: pageError(answer.err) };
      var choices = answer.ok && Array.isArray(answer.ok.choices) ? answer.ok.choices : [];
      if (!choices.length) return { err: { code: 'no_match', message: '' } };
      var pick = choices.length === 1 ? Promise.resolve({ choice: 0 }) : choose(f, {
        what: 'passkey',
        site: rpId,
        items: choices.map(function (c) { return { name: c.user_display || c.user_name || '', detail: c.user_name || '' }; }),
        browser: true
      });
      return pick.then(function (picked) {
        if (f.aborted) return { err: { code: 'aborted', message: '' } };
        if (picked.browser) return { err: { code: 'browser', message: 'The person chose the browser’s own.' } };
        var chosen = typeof picked.choice === 'number' ? choices[picked.choice] : null;
        if (!chosen) return { err: { code: 'denied', message: 'No account was chosen.' } };
        // Only the chosen account's answer goes to the page.
        return {
          ok: {
            credential_id: chosen.credential_id,
            client_data_json: b64url(utf8(data)),
            authenticator_data: chosen.authenticator_data,
            signature: chosen.signature,
            user_handle: chosen.user_handle
          }
        };
      });
    });
  }

  function webauthn(message, sender) {
    var request = message.request && typeof message.request === 'object' ? message.request : {};
    var origin = originOf(sender);
    if (!origin || !isSecure(origin)) return Promise.resolve({ err: { code: 'browser', message: 'Not a secure page.' } });
    if (crossFrame(sender, origin, message.cross === true)) return Promise.resolve({ err: { code: 'browser', message: 'A frame of another site.' } });
    if (request.t !== 'create' && request.t !== 'get') return Promise.resolve({ err: { code: 'bad_request', message: 'Not a request.' } });
    // One at a time in a tab, as browsers have it.
    if (busyTab(sender.tab.id)) return Promise.resolve({ err: { code: 'pending', message: 'A request is already pending.' } });
    var key = keyOf(sender, message, request.id);
    var f = frameOf(sender);
    inflight.set(key, f);
    var answer = request.t === 'create' ? createPasskey(request, origin, f) : getPasskey(request, origin, f);
    return answer.then(function (said) {
      inflight.delete(key);
      return said;
    }, function (error) {
      inflight.delete(key);
      return { err: { code: 'browser', message: String((error && error.message) || error) } };
    });
  }

  function abort(message, sender) {
    var f = inflight.get(keyOf(sender, message, message.id));
    if (f) {
      f.aborted = true;
      if (f.choosing) Promise.resolve(api.tabs.sendMessage(f.tab, { k: 'close' }, target(f))).catch(function () {});
    }
    return Promise.resolve({});
  }

  // ---- logins ----
  function fill(message, sender) {
    var origin = originOf(sender);
    if (!origin || !isSecure(origin)) return Promise.resolve({ err: { code: 'insecure', message: 'Not a secure page.' } });
    return native({ op: 'fill', origin: origin }).then(function (answer) {
      if (answer.err) return { err: answer.err };
      var logins = answer.ok && Array.isArray(answer.ok.logins) ? answer.ok.logins : [];
      if (!logins.length) return { none: true };
      var pick = logins.length === 1 ? Promise.resolve({ choice: 0 }) : choose(frameOf(sender), {
        what: 'login',
        site: hostOf(origin),
        items: logins.map(function (l) { return { name: l.title || l.username || hostOf(origin), detail: l.username || '' }; })
      });
      return pick.then(function (picked) {
        var chosen = typeof picked.choice === 'number' ? logins[picked.choice] : null;
        var out = chosen ? { login: { username: chosen.username || '', password: chosen.password || '', totp: chosen.totp || null } } : { cancel: true };
        // Let go of every password but the one going to the page.
        logins.forEach(function (l) { l.password = ''; });
        return out;
      });
    });
  }

  // Logins the person has sent, by tab, waiting for a yes or a no.
  var offers = new Map();

  function forget(tab, token) {
    var offer = offers.get(tab);
    if (!offer || (token && offer.token !== token)) return false;
    offers.delete(tab);
    clearTimeout(offer.timer);
    offer.password = '';
    release();
    return true;
  }

  function view(offer) {
    return { token: offer.token, username: offer.username, site: hostOf(offer.origin), kind: offer.kind };
  }

  function randomToken() {
    var bytes = new Uint8Array(16);
    crypto.getRandomValues(bytes);
    return b64url(bytes);
  }

  function submitted(message, sender) {
    var origin = originOf(sender);
    if (!origin || !isSecure(origin) || typeof message.password !== 'string' || !message.password) return Promise.resolve({});
    var tab = sender.tab.id;
    forget(tab);
    var offer = {
      token: randomToken(),
      origin: origin,
      frame: sender.frameId || 0,
      username: typeof message.username === 'string' ? message.username : '',
      password: message.password,
      kind: message.kind === 'new-password' ? 'new-password' : 'password',
      timer: 0
    };
    offers.set(tab, offer);
    offer.timer = setTimeout(function () { forget(tab, offer.token); }, OFFER_MS);
    hold();
    // In the top frame, where it is seen; in the sending frame when the top one is not ours.
    var shown = { k: 'offer', offer: view(offer) };
    Promise.resolve(api.tabs.sendMessage(tab, shown, { frameId: 0 })).catch(function () {
      if (offer.frame) return api.tabs.sendMessage(tab, shown, { frameId: offer.frame });
    }).catch(function () {});
    return Promise.resolve({});
  }

  function pendingOffer(message, sender) {
    var offer = offers.get(sender.tab.id);
    return Promise.resolve(offer && !sender.frameId ? { offer: view(offer) } : {});
  }

  function save(message, sender) {
    var tab = sender.tab.id, offer = offers.get(tab);
    if (!offer || offer.token !== message.token) return Promise.resolve({ err: { code: 'gone', message: 'That login is no longer here.' } });
    var request = { op: 'save', origin: offer.origin, title: '', username: offer.username, password: offer.password };
    forget(tab);
    return native(request).then(function (answer) {
      request.password = '';
      return answer.err ? { err: answer.err } : { ok: true };
    });
  }

  function dismiss(message, sender) {
    forget(sender.tab.id, message.token);
    return Promise.resolve({});
  }

  var HANDLERS = { webauthn: webauthn, abort: abort, fill: fill, submitted: submitted, offers: pendingOffer, save: save, dismiss: dismiss };

  api.runtime.onMessage.addListener(function (message, sender, respond) {
    if (!message || typeof message.k !== 'string' || !HANDLERS[message.k]) return false;
    // Only this extension's own content scripts, in a tab.
    if (!sender || !sender.tab || (sender.id && sender.id !== api.runtime.id)) return false;
    HANDLERS[message.k](message, sender).then(function (answer) {
      respond(answer || {});
    }, function (error) {
      respond({ err: { code: 'failed', message: String((error && error.message) || error) } });
    });
    return true;
  });

  if (api.tabs && api.tabs.onRemoved) api.tabs.onRemoved.addListener(function (tab) { forget(tab); });

  if (testing) {
    testing.originOf = originOf;
    testing.hostOf = hostOf;
    testing.isSecure = isSecure;
    testing.rpIdOk = rpIdOk;
    testing.clientDataJSON = clientDataJSON;
    testing.sha256 = sha256;
    testing.b64url = b64url;
    testing.pageError = pageError;
    testing.browserName = browserName;
  }
})();
