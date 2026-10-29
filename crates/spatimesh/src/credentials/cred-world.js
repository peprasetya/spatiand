// SpatiKey's relay in SpatiMesh (docs/spatikey.md, "SpatiMesh"). Run in every frame at the start of
// every document, in an isolated world of its own, "spatimesh-cred", with extensions/spatikey's
// forms.js put before it. It is the only script that can reach the window's message handler
// `spatimeshCred`: a page cannot post to it, cannot see this world's globals, and cannot change
// what `self.origin` returns here. So the origin stamped on everything it passes on is the
// engine's word, not the page's, and the window builds clientDataJSON from it.
//
// It posts JSON strings to webkit.messageHandlers.spatimeshCred, a handler with reply: each post's
// promise is the window's answer.
//
//   webauthn.js's requests ({id, t:"create"|"get"|"abort", ...}, as events on the document) go up
//   stamped with `origin` (self.origin), `main` (is this the top frame), `frame` (this document's
//   own random name, so that the window can tell frames of one origin apart), `top_origin` (the
//   top frame's origin, when this frame can read it) and `ancestors` (the origins of the frames
//   around this one, nearest first, as location.ancestorOrigins has them, where it is there). The
//   answer, {ok} or {err}, goes back to webauthn.js as "spatikey-reply". An abort is answered by
//   nobody.
//
//   {t:"ready", origin, main, frame, ...} is always waiting, one per document: its answer is how
//   the window tells this frame something later -- {fill:{username, password, totp, field}} into
//   the login the person is in, {conditional:{id, ok|err}} the answer to a conditional get once
//   the person has chosen with the key button (the window may instead answer that get's own post,
//   whichever comes first), or {stop:true} to send no more. After each answer another goes.
//
//   forms.js's reports ({t:"fields"|"submitted", ...}) go up stamped the same way, unanswered.
(function () {
  'use strict';
  if (window.__spatikeyCredWorld) return;
  // This world's own global: the page never sees it.
  window.__spatikeyCredWorld = true;
  var messageHandlers = window.webkit && window.webkit.messageHandlers;
  var handler = messageHandlers && messageHandlers.spatimeshCred;
  if (!handler || typeof handler.postMessage !== 'function') return;
  var doc = document;
  var main = window.top === window;
  var STAMPED = { origin: true, main: true, frame: true, top_origin: true, ancestors: true };

  function randomName() {
    var bytes = new Uint8Array(8), out = '';
    crypto.getRandomValues(bytes);
    for (var i = 0; i < bytes.length; i++) out += (bytes[i] < 16 ? '0' : '') + bytes[i].toString(16);
    return out;
  }
  var frame = randomName();

  function ancestors() {
    var list = location.ancestorOrigins;
    if (!list) return null;
    var out = [];
    for (var i = 0; i < list.length; i++) out.push(String(list[i]));
    return out;
  }

  function topOrigin(around) {
    try {
      return String(window.top.location.origin);
    } catch (e) {
      // The top frame is another site's: the engine still says what it is, through
      // ancestorOrigins, whose last entry is the top frame's.
      return around && around.length ? around[around.length - 1] : null;
    }
  }

  // `message`, with what this world knows of the frame put over anything the page said of it.
  function stamp(message) {
    var out = {}, names = Object.keys(message);
    for (var i = 0; i < names.length; i++) if (!STAMPED[names[i]]) out[names[i]] = message[names[i]];
    out.origin = String(self.origin);
    out.main = main;
    out.frame = frame;
    var around = ancestors();
    var top = topOrigin(around);
    if (top) out.top_origin = top;
    if (around) out.ancestors = around;
    return out;
  }

  // The window's answer to `message` as a promise, or null when the handler gives none (one
  // registered without reply).
  function post(message) {
    var answer;
    try {
      answer = handler.postMessage(JSON.stringify(stamp(message)));
    } catch (e) {
      return Promise.reject(e);
    }
    return answer && typeof answer.then === 'function' ? answer : null;
  }

  function quietly(promise) {
    if (promise) promise.then(null, function () {});
  }

  function read(value) {
    if (typeof value === 'string') {
      try {
        value = JSON.parse(value);
      } catch (e) {
        return null;
      }
    }
    return value && typeof value === 'object' ? value : null;
  }

  function settled(answer) {
    return !!answer && ((answer.ok !== undefined && answer.ok !== null) || (answer.err !== undefined && answer.err !== null));
  }

  // ---- webauthn.js's requests ----
  // Conditional gets waiting for the person, by id: their answer may come through "ready".
  var waiting = Object.create(null);
  var lastConditional = null;

  function answer(id, said) {
    delete waiting[id];
    if (lastConditional === id) lastConditional = null;
    var out = { id: id };
    if (said.err !== undefined && said.err !== null) out.err = said.err;
    else out.ok = said.ok;
    doc.dispatchEvent(new CustomEvent('spatikey-reply', { detail: JSON.stringify(out) }));
  }

  function failed(id, message) {
    answer(id, { err: { code: 'failed', message: message } });
  }

  doc.addEventListener('spatikey-request', function (ev) {
    var request = read(typeof ev.detail === 'string' ? ev.detail : null);
    if (!request || typeof request.t !== 'string') return;
    var id = request.id;
    if (typeof id !== 'number' && typeof id !== 'string') return;
    if (request.t === 'abort') {
      delete waiting[id];
      if (lastConditional === id) lastConditional = null;
      quietly(post(request));
      return;
    }
    var conditional = request.t === 'get' && request.conditional === true;
    if (conditional) {
      waiting[id] = true;
      lastConditional = id;
    }
    var reply = post(request);
    if (!reply) {
      failed(id, 'SpatiMesh did not take the request.');
      return;
    }
    reply.then(function (value) {
      var said = read(value);
      if (settled(said)) {
        // Answered already through "ready", or aborted: webauthn.js has stopped listening for it.
        if (conditional && !waiting[id]) return;
        answer(id, said);
      } else if (!conditional) {
        failed(id, 'SpatiMesh gave no answer.');
      }
    }, function (error) {
      if (conditional && !waiting[id]) return;
      failed(id, String((error && error.message) || error || 'SpatiMesh refused the request.'));
    });
  }, true);

  // ---- forms ----
  var forms = typeof SpatiKeyForms === 'function' ? SpatiKeyForms({
    report: function (message) { quietly(post(message)); },
    inPageButton: false
  }) : null;

  // ---- what the window says later ----
  var stopped = false;

  function command(said) {
    if (said.fill && typeof said.fill === 'object' && forms) forms.fill(said.fill);
    if (said.conditional && typeof said.conditional === 'object') {
      var c = said.conditional;
      var id = c.id !== undefined && c.id !== null ? c.id : lastConditional;
      if (id !== null && waiting[id] && settled(c)) answer(id, c);
    }
  }

  function ready() {
    if (stopped) return;
    var asked = Date.now();
    var reply = post({ t: 'ready' });
    // A handler without reply cannot say anything later: nothing to wait for.
    if (!reply) return;
    reply.then(function (value) {
      var said = read(value) || {};
      try {
        command(said);
      } catch (e) {}
      if (said.stop) {
        stopped = true;
        if (forms) forms.stop();
        return;
      }
      // An empty answer straight back, again and again, would spin: a breath before the next.
      if (!said.fill && !said.conditional && Date.now() - asked < 200) setTimeout(ready, 1000);
      else ready();
    }, function () {
      // The window has gone, or let this document go.
      stopped = true;
    });
  }

  ready();
})();
