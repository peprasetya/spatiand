// SpatiKey's extension in the page: its isolated world, in every frame from the start of every
// document, with forms.js before it (docs/spatikey.md, "The extensions"). The page cannot reach
// this world's globals or change its built-ins; it shares only the DOM.
//
// It relays webauthn.js's requests (events on the document) to the background, which asks
// SpatiKey with the origin the browser gives it, and hands the answers back as "spatikey-reply".
// A conditional get waits here until the person presses the key button in a login field. The key
// button (forms.js's) fills the field from SpatiKey, with a chooser when the site has several
// logins; a login the person typed and sent is offered for saving, in a small bar. Both are drawn
// in closed shadow roots, out of the page's reach, and act only on the person's own clicks.
(function () {
  'use strict';
  if (window.__spatikeyContent) return;
  window.__spatikeyContent = true;
  var api = typeof browser === 'object' && browser && browser.runtime ? browser : chrome;
  var doc = document;
  var top = window.top === window;
  // This document's own name, so that the background can tell a request from one before a reload.
  var docName = Math.random().toString(36).slice(2) + Date.now().toString(36);

  function ask(message) {
    message.doc = docName;
    try {
      return Promise.resolve(api.runtime.sendMessage(message));
    } catch (e) {
      // The extension was reloaded or removed under this page.
      return Promise.reject(e);
    }
  }

  // Is this frame another site's, inside the page? Read here, where the page cannot fake it: a
  // parent of another origin cannot be looked into at all.
  var crossKnown = null;
  function crossOrigin() {
    if (crossKnown !== null) return crossKnown;
    crossKnown = false;
    if (top) return crossKnown;
    try {
      var mine = location.origin;
      for (var w = window; w !== window.top;) {
        w = w.parent;
        if (w.location.origin !== mine) {
          crossKnown = true;
          break;
        }
      }
    } catch (e) {
      crossKnown = true;
    }
    return crossKnown;
  }

  // ---- webauthn.js's requests ----
  var conditional = null;   // a conditional get, waiting for the key button
  var busy = false;

  function reply(id, said) {
    var out = { id: id };
    if (said && said.ok) out.ok = said.ok;
    else out.err = (said && said.err) || { code: 'browser', message: 'SpatiKey gave no answer.' };
    doc.dispatchEvent(new CustomEvent('spatikey-reply', { detail: JSON.stringify(out) }));
  }

  function relay(request) {
    ask({ k: 'webauthn', request: request, cross: crossOrigin() }).then(function (said) {
      reply(request.id, said);
    }, function (error) {
      reply(request.id, { err: { code: 'browser', message: String((error && error.message) || error) } });
    });
  }

  doc.addEventListener('spatikey-request', function (ev) {
    var request;
    try {
      request = typeof ev.detail === 'string' ? JSON.parse(ev.detail) : null;
    } catch (e) {
      return;
    }
    if (!request || typeof request !== 'object' || typeof request.t !== 'string') return;
    if (typeof request.id !== 'number' && typeof request.id !== 'string') return;
    if (request.t === 'abort') {
      if (conditional && conditional.id === request.id) conditional = null;
      ask({ k: 'abort', id: request.id }).catch(function () {});
      return;
    }
    if (request.t === 'get' && request.conditional === true) {
      conditional = request;
      return;
    }
    if (request.t === 'create' || request.t === 'get') relay(request);
  }, true);

  // ---- what is drawn: a chooser, the save bar, a note ----
  var dark = !!(window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches);
  var C = dark
    ? { bg: '#22262e', fg: '#eceff4', muted: '#a3abb9', line: 'rgba(255,255,255,0.14)', hover: 'rgba(255,255,255,0.08)', accent: '#7aa7ff', onAccent: '#0b1220' }
    : { bg: '#ffffff', fg: '#1f2329', muted: '#5f6673', line: 'rgba(0,0,0,0.12)', hover: 'rgba(0,0,0,0.05)', accent: '#2563eb', onAccent: '#ffffff' };
  var FONT = '13px/1.4 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif';

  // Styles go one property at a time through the CSSOM: a page's Content-Security-Policy can
  // forbid style sheets and style attributes, but not this.
  function css(el, styles) {
    for (var name in styles) if (Object.prototype.hasOwnProperty.call(styles, name)) el.style.setProperty(name, styles[name], 'important');
    return el;
  }

  function make(tag, styles, text) {
    var el = doc.createElement(tag);
    if (styles) css(el, styles);
    if (text) el.textContent = text;
    return el;
  }

  function whenReady(fn) {
    if (doc.documentElement && doc.body) fn();
    else doc.addEventListener('DOMContentLoaded', fn, { once: true });
  }

  // A box of our own above the page, in a closed shadow root.
  function panel(name, placeAt) {
    var host = doc.createElement(name);
    css(host, { all: 'initial', position: 'fixed', 'z-index': '2147483647', display: 'block' });
    var root = host.attachShadow({ mode: 'closed' });
    var box = make('div', {
      all: 'initial', display: 'block', 'box-sizing': 'border-box', font: FONT, color: C.fg, background: C.bg,
      border: '1px solid ' + C.line, 'border-radius': '10px', 'box-shadow': '0 8px 28px rgba(0,0,0,0.22)', padding: '10px',
      width: '300px', 'max-width': 'calc(100vw - 16px)'
    });
    root.appendChild(box);
    var rect = placeAt && placeAt.width ? placeAt : null;
    if (rect) {
      var left = Math.max(8, Math.min(rect.left, window.innerWidth - 308));
      var below = rect.bottom + 6;
      css(host, { left: left + 'px', top: (below + 200 > window.innerHeight ? Math.max(8, rect.top - 206) : below) + 'px' });
    } else {
      css(host, { right: '12px', top: '12px' });
    }
    doc.documentElement.appendChild(host);
    return { host: host, box: box };
  }

  function title(text) {
    var row = make('div', { display: 'flex', 'align-items': 'center', gap: '6px', 'margin-bottom': '8px', 'font-weight': '600' });
    var mark = make('span', { color: C.accent, 'font-weight': '700' }, 'SpatiKey');
    row.appendChild(mark);
    if (text) row.appendChild(make('span', { color: C.muted, 'font-weight': '400', overflow: 'hidden', 'text-overflow': 'ellipsis', 'white-space': 'nowrap' }, text));
    return row;
  }

  function actionButton(label, primary) {
    var b = make('button', {
      all: 'initial', font: FONT, 'font-weight': '600', cursor: 'pointer', padding: '5px 12px', 'border-radius': '6px',
      color: primary ? C.onAccent : C.fg, background: primary ? C.accent : 'transparent', border: '1px solid ' + (primary ? C.accent : C.line)
    }, label);
    b.type = 'button';
    return b;
  }

  // Only the person's own clicks and keys count: never a script's.
  function onPress(el, fn) {
    el.addEventListener('click', function (ev) {
      ev.stopPropagation();
      if (ev.isTrusted) fn(ev);
    });
  }

  // ---- the chooser ----
  var chooser = null;

  function closeChooser(answer) {
    if (!chooser) return;
    var c = chooser;
    chooser = null;
    doc.removeEventListener('mousedown', c.outside, true);
    c.host.remove();
    try {
      c.respond(answer || { cancel: true });
    } catch (e) {}
  }

  function showChooser(message, respond) {
    closeChooser();
    whenReady(function () {
      var p = panel('spatikey-choose', forms ? forms.place() : null);
      var heading = message.what === 'passkey' ? 'Sign in to ' + (message.site || location.hostname) : 'Fill a login for ' + (message.site || location.hostname);
      p.box.appendChild(title(heading));
      var list = make('div', { display: 'flex', 'flex-direction': 'column', gap: '2px' });
      var buttons = [];
      var items = Array.isArray(message.items) ? message.items : [];
      items.forEach(function (item, index) {
        var b = make('button', {
          all: 'initial', display: 'block', 'box-sizing': 'border-box', width: '100%', padding: '7px 9px', 'border-radius': '7px',
          cursor: 'pointer', font: FONT, color: C.fg, background: 'transparent', 'text-align': 'left', outline: 'none'
        });
        b.type = 'button';
        b.appendChild(make('div', { 'font-weight': '600', overflow: 'hidden', 'text-overflow': 'ellipsis', 'white-space': 'nowrap' }, String(item.name || item.detail || 'Account')));
        if (item.detail && item.detail !== item.name) {
          b.appendChild(make('div', { color: C.muted, overflow: 'hidden', 'text-overflow': 'ellipsis', 'white-space': 'nowrap' }, String(item.detail)));
        }
        var lit = function (on) { css(b, { background: on ? C.hover : 'transparent' }); };
        b.addEventListener('mouseenter', function () { lit(true); });
        b.addEventListener('mouseleave', function () { lit(doc.activeElement === b); });
        b.addEventListener('focus', function () { lit(true); });
        b.addEventListener('blur', function () { lit(false); });
        onPress(b, function () { closeChooser({ choice: index }); });
        buttons.push(b);
        list.appendChild(b);
      });
      p.box.appendChild(list);
      var row = make('div', { display: 'flex', 'justify-content': 'flex-end', gap: '8px', 'margin-top': '8px' });
      if (message.browser) {
        var other = actionButton('Use the browser’s own', false);
        onPress(other, function () { closeChooser({ browser: true }); });
        buttons.push(other);
        row.appendChild(other);
      }
      var cancel = actionButton('Cancel', false);
      onPress(cancel, function () { closeChooser({ cancel: true }); });
      buttons.push(cancel);
      row.appendChild(cancel);
      p.box.appendChild(row);
      p.box.addEventListener('keydown', function (ev) {
        if (!ev.isTrusted) return;
        var at = buttons.indexOf(ev.composedPath()[0]);
        if (ev.key === 'Escape') {
          ev.preventDefault();
          closeChooser({ cancel: true });
        } else if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
          ev.preventDefault();
          var next = (at < 0 ? 0 : at + (ev.key === 'ArrowDown' ? 1 : buttons.length - 1)) % buttons.length;
          buttons[next].focus();
        }
        ev.stopPropagation();
      });
      var outside = function (ev) {
        if (ev.isTrusted && ev.composedPath()[0] !== p.host && ev.composedPath().indexOf(p.host) < 0) closeChooser({ cancel: true });
      };
      doc.addEventListener('mousedown', outside, true);
      chooser = { host: p.host, respond: respond, outside: outside };
      if (buttons.length) buttons[0].focus({ preventScroll: true });
    });
  }

  // ---- notes ----
  var note = null;

  function showNote(text) {
    whenReady(function () {
      if (note) note.host.remove();
      var p = panel('spatikey-note', forms ? forms.place() : null);
      p.box.appendChild(title(''));
      p.box.appendChild(make('div', null, text));
      note = p;
      setTimeout(function () {
        if (note === p) {
          p.host.remove();
          note = null;
        }
      }, 5000);
    });
  }

  var WHY = {
    unavailable: 'SpatiKey’s helper is not installed for this browser, or SpatiKey is not running (tools/spatikey-browsers.sh installs it).',
    no_vault: 'SpatiKey is not set up yet: open SpatiKey on the Mac.',
    no_approver: 'This device has no Mac to approve with: pair it with SpatiKey on the Mac first.',
    not_reachable: 'No Mac answered on this network.',
    not_same_lan: 'The Mac refused: this device is not on its network.',
    touch_id_unavailable: 'Touch ID cannot be used now: is the Mac locked, or its lid shut?',
    fingerprints_changed: 'The Mac’s fingerprints changed: set Touch ID up again with the recovery code.',
    timeout: 'The Mac did not answer in time.',
    insecure: 'SpatiKey fills secure pages only (https, or this computer’s own).',
    gone: 'That login is no longer here: sign in again to save it.'
  };

  function explain(err) {
    var code = (err && err.code) || 'failed';
    // The person said no: they know.
    if (code === 'denied') return;
    showNote(WHY[code] || (err && err.message) || 'SpatiKey could not do that.');
  }

  // ---- the key button: a passkey when the page waits for one, else a login ----
  function fillLogin(field) {
    busy = true;
    ask({ k: 'fill' }).then(function (said) {
      busy = false;
      said = said || {};
      if (said.login) {
        forms.fill({ username: said.login.username, password: said.login.password, totp: said.login.totp, field: field });
        said.login = null;
      } else if (said.none) {
        showNote('SpatiKey has no login for ' + location.hostname + '.');
      } else if (!said.cancel) {
        explain(said.err);
      }
    }, function () {
      busy = false;
      explain({ code: 'unavailable' });
    });
  }

  function onKey(field) {
    if (busy) return;
    var request = conditional;
    if (!request) {
      fillLogin(field);
      return;
    }
    busy = true;
    ask({ k: 'webauthn', request: request, cross: crossOrigin() }).then(function (said) {
      busy = false;
      if (conditional !== request) return;
      if (said && said.ok) {
        conditional = null;
        reply(request.id, said);
        return;
      }
      var code = said && said.err && said.err.code;
      // No passkey here for this site: the key button means a password, then.
      if (code === 'no_match' || code === 'browser') fillLogin(field);
      // Otherwise the get goes on waiting, as a conditional one does: another press tries again.
      else explain(said && said.err);
    }, function () {
      busy = false;
      explain({ code: 'unavailable' });
    });
  }

  // ---- saving what the person typed ----
  var bar = null;

  function closeBar() {
    if (!bar) return;
    clearTimeout(bar.timer);
    bar.host.remove();
    bar = null;
  }

  function showOffer(offer) {
    if (!offer || !offer.token) return;
    whenReady(function () {
      if (bar && bar.token === offer.token) return;
      closeBar();
      var p = panel('spatikey-save', null);
      var mine = { host: p.host, token: offer.token, timer: 0 };
      bar = mine;
      p.box.appendChild(title(offer.site || ''));
      var who = offer.username ? ' for ' + offer.username : '';
      var question = make('div', { 'margin-bottom': '10px' }, (offer.kind === 'new-password' ? 'Save the new password' : 'Save the password') + who + ' in SpatiKey?');
      p.box.appendChild(question);
      var row = make('div', { display: 'flex', 'justify-content': 'flex-end', gap: '8px' });
      var later = actionButton('Not now', false), save = actionButton('Save', true);
      row.appendChild(later);
      row.appendChild(save);
      p.box.appendChild(row);
      onPress(later, function () {
        ask({ k: 'dismiss', token: offer.token }).catch(function () {});
        closeBar();
      });
      onPress(save, function () {
        if (bar !== mine) return;
        clearTimeout(mine.timer);
        row.remove();
        question.textContent = 'Approve with Touch ID on your Mac…';
        ask({ k: 'save', token: offer.token }).then(function (said) {
          if (bar !== mine) return;
          if (said && said.ok) question.textContent = 'Saved in SpatiKey.';
          else if (said && said.err && said.err.code === 'denied') question.textContent = 'Not saved.';
          else question.textContent = WHY[(said && said.err && said.err.code) || ''] || (said && said.err && said.err.message) || 'SpatiKey could not save it.';
          mine.timer = setTimeout(function () { if (bar === mine) closeBar(); }, 4000);
        }, function () {
          if (bar !== mine) return;
          question.textContent = WHY.unavailable;
          mine.timer = setTimeout(function () { if (bar === mine) closeBar(); }, 6000);
        });
      });
      // Unanswered, it goes after a minute, as if "Not now".
      mine.timer = setTimeout(function () {
        if (bar !== mine) return;
        ask({ k: 'dismiss', token: offer.token }).catch(function () {});
        closeBar();
      }, 60000);
    });
  }

  // ---- forms ----
  var forms = typeof SpatiKeyForms === 'function' ? SpatiKeyForms({
    inPageButton: true,
    report: function (message) {
      if (message.t === 'key') onKey(message.field);
      else if (message.t === 'submitted') {
        ask({ k: 'submitted', username: message.username, password: message.password, kind: message.kind }).catch(function () {});
      }
      // "fields" is for SpatiMesh's window, which has its own button: here the field's own does it.
    }
  }) : null;

  // ---- what the background says ----
  api.runtime.onMessage.addListener(function (message, sender, respond) {
    if (!message || typeof message.k !== 'string') return false;
    if (message.k === 'choose') {
      showChooser(message, respond);
      return true;
    }
    if (message.k === 'close') closeChooser({ cancel: true });
    else if (message.k === 'offer') showOffer(message.offer);
    return false;
  });

  // A login sent from the page before this one (a form that navigated): the bar is offered here.
  if (top) {
    ask({ k: 'offers' }).then(function (said) {
      if (said && said.offer) showOffer(said.offer);
    }, function () {});
  }
})();
