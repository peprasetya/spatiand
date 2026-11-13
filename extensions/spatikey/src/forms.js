// SpatiKey's half of a page's login forms (docs/spatikey.md), for the two relays that run in an
// isolated world beside the page: SpatiMesh's "spatimesh-cred" world, where cred-world.js follows
// this file, and the extension's content script, where content.js does. An isolated world shares
// the page's DOM but not its JavaScript, so the page can neither change what these functions do
// nor read what they keep.
//
// SpatiKeyForms({report, inPageButton}) watches this frame's document and calls `report` with
//
//   {t:"fields", kind, field, webauthn}   a login field has the focus: kind "username",
//                                         "password", "new-password" or "one-time-code", or ""
//                                         once none has; field is its number in this frame
//   {t:"submitted", username, password, kind}
//                                         a login the person typed is being sent (kind
//                                         "new-password" for a sign-up or a change)
//   {t:"key", field}                      the key button in the field was pressed
//
// and returns {fill, field, place, stop}. fill({username, password, totp, field}) puts a login
// into the login form the person is in -- the field they were last in, or `field` -- in this frame
// only, as if typed (the input element's own value setter, then input and change events, which
// is what React and the like listen for). The key button is the extension's (`inPageButton`);
// SpatiMesh has one in its own window instead.
function SpatiKeyForms(options) {
  'use strict';
  options = options || {};
  var report = typeof options.report === 'function' ? options.report : function () {};
  var withButton = !!options.inPageButton;
  var doc = document, win = window;
  var valueSetter = (Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value') || {}).set;
  // Through the prototype: a form's own fields are properties of it, and one named "elements"
  // would stand in for the real thing.
  var formElements = Object.getOwnPropertyDescriptor(HTMLFormElement.prototype, 'elements').get;

  var numbers = new WeakMap(), byNumber = new Map(), nextNumber = 1;
  var current = null;               // the login field that has the focus, or last had it
  var said = '';                    // the last "fields" report, to say each change once
  var wasPassword = new WeakSet();  // fields seen as type=password (a "show password" button retypes them)
  var typed = new WeakSet();        // fields the person typed or pasted into
  var lastTyped = null;             // the password field typed into last
  var filledWith = new WeakMap();   // field -> what SpatiKey put there
  var lastUsername = '';            // the account last typed or filled in this frame (two-step logins)
  var lastReport = '';
  var stopped = false;

  // ---- which fields are login fields ----
  var TEXTLIKE = { text: true, email: true, tel: true };

  function isInput(el) {
    return !!el && el.nodeType === 1 && el.localName === 'input';
  }

  function tokens(el) {
    return (el.getAttribute('autocomplete') || '').toLowerCase().split(/\s+/);
  }

  function has(list, word) {
    return list.indexOf(word) >= 0;
  }

  function isPassword(el) {
    if (el.type === 'password') {
      wasPassword.add(el);
      return true;
    }
    return wasPassword.has(el);
  }

  function kindOf(el) {
    if (!isInput(el) || el.disabled) return '';
    var t = tokens(el);
    if (has(t, 'one-time-code')) return 'one-time-code';
    if (isPassword(el)) return has(t, 'new-password') ? 'new-password' : 'password';
    if (!TEXTLIKE[el.type]) return '';
    if (has(t, 'new-password')) return 'new-password';
    if (has(t, 'current-password')) return 'password';
    if (has(t, 'username') || has(t, 'webauthn')) return 'username';
    if (passwordAfter(el)) return 'username';
    if (looksLikeCode(el)) return 'one-time-code';
    return '';
  }

  // A code field with no autocomplete of its own: numeric, short, and named for what it is.
  function looksLikeCode(el) {
    if (el.getAttribute('inputmode') !== 'numeric' && el.type !== 'tel') return false;
    var max = el.maxLength;
    if (max > 0 && (max < 4 || max > 10)) return false;
    var words = (el.name || '') + ' ' + (el.id || '') + ' ' + (el.getAttribute('aria-label') || '') + ' ' + (el.placeholder || '');
    return /\b(otp|totp|2fa|mfa)\b|one.?time|verification.?code|security.?code|auth\w*.?code/i.test(words);
  }

  function scopeOf(el) {
    return el.form || doc;
  }

  function inputsIn(scope) {
    var list = scope === doc ? doc.querySelectorAll('input') : formElements.call(scope), out = [];
    for (var i = 0; i < list.length; i++) if (isInput(list[i]) && list[i].type !== 'hidden') out.push(list[i]);
    return out;
  }

  // Is there a password field soon after `el`, in its form (or the page, with no form)? Hidden
  // ones count: a two-step login keeps one on its first page for password managers to see.
  function passwordAfter(el) {
    var list = inputsIn(scopeOf(el)), seen = false, steps = 0;
    for (var i = 0; i < list.length; i++) {
      if (list[i] === el) {
        seen = true;
        continue;
      }
      if (!seen) continue;
      if (isPassword(list[i])) return true;
      if (++steps > 3) return false;
    }
    return false;
  }

  function visible(el) {
    if (!el || !el.isConnected) return false;
    if (typeof el.checkVisibility === 'function') {
      if (!el.checkVisibility({ opacityProperty: true, visibilityProperty: true, checkOpacity: true, checkVisibilityCSS: true })) return false;
    } else if (!el.getClientRects().length) {
      return false;
    }
    var r = el.getBoundingClientRect();
    return r.width >= 2 && r.height >= 2;
  }

  function numberOf(el) {
    var n = numbers.get(el);
    if (!n) {
      n = nextNumber++;
      numbers.set(el, n);
      byNumber.set(n, el);
      if (byNumber.size > 64) byNumber.forEach(function (field, key) { if (!field.isConnected) byNumber.delete(key); });
    }
    return n;
  }

  function targetOf(ev) {
    var path = typeof ev.composedPath === 'function' ? ev.composedPath() : null;
    return path && path.length ? path[0] : ev.target;
  }

  function deepActive() {
    var a = doc.activeElement;
    while (a && a.shadowRoot && a.shadowRoot.activeElement) a = a.shadowRoot.activeElement;
    return a;
  }

  function tell(message) {
    if (stopped) return;
    try {
      report(message);
    } catch (e) {}
  }

  // ---- the focus ----
  function say(kind, el) {
    var field = el ? numberOf(el) : 0;
    var key = kind + ':' + field;
    if (key === said) return;
    said = key;
    tell({ t: 'fields', kind: kind, field: field, webauthn: !!el && has(tokens(el), 'webauthn') });
  }

  function onFocusIn(ev) {
    var el = targetOf(ev);
    if (button && el === button.host) return;
    var kind = kindOf(el);
    if (kind) {
      current = el;
      say(kind, el);
      showButton(el);
    } else if (said && said.charAt(0) !== ':') {
      say('', null);
      hideButton();
    }
  }

  function onFocusOut(ev) {
    var el = targetOf(ev);
    if (el !== current) return;
    if (kindOf(el) === 'username' && el.value) lastUsername = el.value;
    if (ev.relatedTarget) return;
    setTimeout(function () {
      // Gone to the window around the page (SpatiMesh's key button, the address bar): the field is
      // still the one the person means.
      if (stopped || !doc.hasFocus()) return;
      var active = deepActive();
      if (!kindOf(active) && !(button && active === button.host)) {
        say('', null);
        hideButton();
      }
    }, 0);
  }

  // ---- typing, and logins being sent ----
  function onInput(ev) {
    if (!ev.isTrusted) return;
    var el = targetOf(ev), kind = kindOf(el);
    if (kind === 'password' || kind === 'new-password') {
      typed.add(el);
      lastTyped = el;
      filledWith.delete(el);
    } else if (kind === 'username') {
      typed.add(el);
      lastUsername = el.value;
    }
    if (button && button.field === el) place();
  }

  function passwordsIn(scope) {
    var list = inputsIn(scope), out = [];
    for (var i = 0; i < list.length; i++) if (isPassword(list[i])) out.push(list[i]);
    return out;
  }

  // The password a submitted form carries: the new one when one was typed (a sign-up, a change of
  // password), else the one typed to sign in. Never one SpatiKey filled and nobody changed, never
  // one only a script set.
  function passwordToReport(scope, preferred) {
    var list = passwordsIn(scope), candidates = [];
    if (preferred && list.indexOf(preferred) < 0) list.push(preferred);
    for (var i = 0; i < list.length; i++) {
      var p = list[i];
      if (!typed.has(p) || !p.value || filledWith.get(p) === p.value) continue;
      candidates.push({ field: p, value: p.value, kind: kindOf(p) === 'new-password' ? 'new-password' : 'password' });
    }
    return SpatiKeyForms.passwordAmong(candidates);
  }

  function usernameIn(scope, before) {
    var list = inputsIn(scope), found = '';
    for (var i = 0; i < list.length; i++) {
      if (list[i] === before) break;
      if (kindOf(list[i]) === 'username' && list[i].value) found = list[i].value;
    }
    if (found) return found;
    for (; i < list.length; i++) if (kindOf(list[i]) === 'username' && list[i].value) return list[i].value;
    return '';
  }

  function consider(scope, preferred) {
    var pw = passwordToReport(scope, preferred);
    if (!pw) return;
    var username = usernameIn(scopeOf(pw.field), pw.field) || lastUsername;
    var key = username + '\n' + pw.value;
    if (key === lastReport) return;
    lastReport = key;
    tell({ t: 'submitted', username: username, password: pw.value, kind: pw.kind });
  }

  function onSubmit(ev) {
    var form = targetOf(ev);
    if (form instanceof HTMLFormElement) consider(form, null);
  }

  var SIGN_IN = /log\s*-?\s*in|log\s*on|sign\s*-?\s*(in|on|up)|submit|continue|next|register|create|save|confirm|anmelden|einloggen|weiter|connexion|connecter|entrar|iniciar|accedi|masuk|daftar|lanjut|ログイン|登录|登入/i;
  var NOT_SIGN_IN = /show|hide|reveal|toggle|visib|forgot|reset/i;

  // The control a click landed on, when it is one a login could be sent with.
  function controlOf(ev) {
    var path = typeof ev.composedPath === 'function' ? ev.composedPath() : [ev.target];
    for (var i = 0; i < path.length && i < 6; i++) {
      var el = path[i];
      if (!el || el.nodeType !== 1) continue;
      if (el.localName === 'button' || (el.localName === 'input' && /^(submit|button|image)$/.test(el.type)) || el.getAttribute('role') === 'button') return el;
    }
    return null;
  }

  function sends(control) {
    var type = (control.getAttribute('type') || '').toLowerCase();
    if (control.localName === 'input' && (type === 'submit' || type === 'image')) return true;
    if (control.localName === 'button' && control.form && (type === 'submit' || type === '')) return true;
    var words = (control.textContent || '') + ' ' + (control.value || '') + ' ' + (control.getAttribute('aria-label') || '') + ' ' +
      (control.id || '') + ' ' + (control.getAttribute('name') || '');
    return SIGN_IN.test(words) && !NOT_SIGN_IN.test(words);
  }

  function onClick(ev) {
    if (!ev.isTrusted) return;
    var control = controlOf(ev);
    if (!control || !sends(control)) return;
    var form = control.form || (control.closest && control.closest('form'));
    consider(form || doc, null);
  }

  function onKeyDown(ev) {
    if (!ev.isTrusted || ev.key !== 'Enter') return;
    var el = targetOf(ev), kind = kindOf(el);
    if (kind === 'password' || kind === 'new-password' || kind === 'username') consider(scopeOf(el), kind === 'username' ? null : el);
  }

  // A login sent by a script that then leaves the page, with no form and no button to see.
  function onPageHide() {
    if (lastTyped && lastTyped.isConnected) consider(scopeOf(lastTyped), lastTyped);
  }

  // ---- filling ----
  function put(el, value) {
    if (el.readOnly || el.disabled) return false;
    el.focus();
    if (valueSetter) valueSetter.call(el, value);
    else el.value = value;
    var Input = typeof InputEvent === 'function' ? InputEvent : Event;
    el.dispatchEvent(new Input('input', { bubbles: true, composed: true, inputType: 'insertReplacementText' }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    filledWith.set(el, value);
    return true;
  }

  function firstLogin() {
    var list = inputsIn(doc), fallback = null;
    for (var i = 0; i < list.length; i++) {
      var kind = kindOf(list[i]);
      if (!kind || !visible(list[i])) continue;
      if (kind === 'password') return list[i];
      if (!fallback && kind === 'username') fallback = list[i];
    }
    return fallback;
  }

  // The field of `kind` that goes with `start`: the nearest one before it for a username, after
  // it for a password, else any visible one in the same form.
  function partner(start, kind) {
    var list = inputsIn(scopeOf(start)), at = list.indexOf(start), i;
    if (kind === 'username') {
      for (i = at - 1; i >= 0; i--) if (kindOf(list[i]) === 'username' && visible(list[i])) return list[i];
    } else {
      for (i = at + 1; i < list.length; i++) if (kindOf(list[i]) === 'password' && visible(list[i])) return list[i];
    }
    var only = null, count = 0;
    for (i = 0; i < list.length; i++) {
      var k = kindOf(list[i]);
      if (list[i] === start || !visible(list[i])) continue;
      if (k === kind) return list[i];
      if (kind === 'password' && k === 'new-password') {
        only = list[i];
        count++;
      }
    }
    // A form whose one password field is marked new-password (some sites do it to keep browsers
    // from filling) is still a login.
    return count === 1 ? only : null;
  }

  function fill(values) {
    values = values || {};
    var start = values.field ? byNumber.get(Number(values.field)) : null;
    if (!start || !start.isConnected) start = current && current.isConnected ? current : firstLogin();
    if (!start) return false;
    var kind = kindOf(start);
    if (kind === 'one-time-code') return values.totp ? put(start, String(values.totp)) : false;
    var userField = kind === 'username' ? start : partner(start, 'username');
    var passField = kind === 'password' || kind === 'new-password' ? start : partner(start, 'password');
    var did = false;
    if (values.username && userField) did = put(userField, String(values.username)) || did;
    if (values.password && passField) did = put(passField, String(values.password)) || did;
    if (values.username) lastUsername = String(values.username);
    var rest = passField || userField;
    if (did && rest) rest.focus();
    return did;
  }

  // ---- the key button, inside the focused field (the extension's) ----
  var button = null;
  var hideTimer = 0;
  var placing = 0;
  var KEY_PATH = 'M7 14a5 5 0 1 1 4.58-7H22v4h-2v3h-3v-3h-5.42A5 5 0 0 1 7 14zm0-3a2 2 0 1 0 0-4 2 2 0 0 0 0 4z';

  function css(el, styles) {
    // Set one property at a time through the CSSOM: a page's Content-Security-Policy may forbid
    // inline style sheets and style attributes, but not this.
    for (var name in styles) if (Object.prototype.hasOwnProperty.call(styles, name)) el.style.setProperty(name, styles[name], 'important');
  }

  function makeButton() {
    var host = doc.createElement('spatikey-key');
    css(host, { all: 'initial', position: 'fixed', 'z-index': '2147483647', display: 'none', margin: '0', padding: '0', border: '0', width: '22px', height: '22px', left: '0', top: '0' });
    var root = host.attachShadow({ mode: 'closed' });
    var b = doc.createElement('button');
    b.type = 'button';
    b.title = 'Fill from SpatiKey';
    b.setAttribute('aria-label', 'Fill from SpatiKey');
    css(b, {
      all: 'initial', display: 'flex', 'align-items': 'center', 'justify-content': 'center', width: '100%', height: '100%',
      'border-radius': '50%', background: 'rgba(37, 99, 235, 0.12)', cursor: 'pointer', padding: '0', margin: '0', 'box-sizing': 'border-box'
    });
    var svg = doc.createElementNS('http://www.w3.org/2000/svg', 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('aria-hidden', 'true');
    css(svg, { width: '68%', height: '68%', display: 'block', 'pointer-events': 'none' });
    var path = doc.createElementNS('http://www.w3.org/2000/svg', 'path');
    path.setAttribute('d', KEY_PATH);
    path.setAttribute('fill', '#2563eb');
    svg.appendChild(path);
    b.appendChild(svg);
    root.appendChild(b);
    b.addEventListener('mouseenter', function () { css(b, { background: 'rgba(37, 99, 235, 0.24)' }); });
    b.addEventListener('mouseleave', function () { css(b, { background: 'rgba(37, 99, 235, 0.12)' }); });
    // Pressing it must not take the focus out of the field it fills.
    b.addEventListener('mousedown', function (ev) { ev.preventDefault(); });
    b.addEventListener('click', function (ev) {
      ev.preventDefault();
      ev.stopPropagation();
      // A script's click is not the person asking.
      if (!ev.isTrusted || !button || !button.field) return;
      tell({ t: 'key', field: numberOf(button.field) });
    });
    return { host: host, field: null };
  }

  function place() {
    placing = 0;
    if (!button || !button.field) return;
    var field = button.field, r = field.getBoundingClientRect();
    if (!visible(field) || r.width < 48 || r.height < 16) {
      css(button.host, { display: 'none' });
      return;
    }
    var size = Math.round(Math.max(16, Math.min(24, r.height - 8)));
    // A page's own icon in the field (show password, clear) sits in a wide right padding: beside it.
    var padding = parseFloat(win.getComputedStyle(field).paddingRight) || 0;
    var inset = padding > size ? padding + 2 : 6;
    css(button.host, {
      display: 'block', width: size + 'px', height: size + 'px',
      left: Math.round(r.right - inset - size) + 'px', top: Math.round(r.top + (r.height - size) / 2) + 'px'
    });
  }

  function placeSoon() {
    if (!placing && button && button.field) placing = win.requestAnimationFrame(place);
  }

  function showButton(el) {
    if (!withButton) return;
    clearTimeout(hideTimer);
    if (!button) button = makeButton();
    button.field = el;
    if (!button.host.isConnected) (doc.documentElement || doc.body).appendChild(button.host);
    place();
  }

  function hideButton() {
    if (!button) return;
    clearTimeout(hideTimer);
    hideTimer = setTimeout(function () {
      if (!button) return;
      button.field = null;
      css(button.host, { display: 'none' });
    }, 150);
  }

  // ---- watching ----
  var listeners = [
    [doc, 'focusin', onFocusIn, true],
    [doc, 'focusout', onFocusOut, true],
    [doc, 'input', onInput, true],
    [doc, 'submit', onSubmit, true],
    [doc, 'click', onClick, true],
    [doc, 'keydown', onKeyDown, true],
    [win, 'pagehide', onPageHide, true],
    [doc, 'scroll', placeSoon, { capture: true, passive: true }],
    [win, 'resize', placeSoon, { passive: true }]
  ];
  listeners.forEach(function (l) { l[0].addEventListener(l[1], l[2], l[3]); });

  // A field that already has the focus when this starts (a page that focuses one on load).
  setTimeout(function () {
    var active = deepActive();
    if (active && kindOf(active)) onFocusIn({ target: active });
  }, 0);

  return {
    fill: fill,
    // The number of the login field the person is in or was last in; 0 for none.
    field: function () { return current && current.isConnected ? numberOf(current) : 0; },
    // Where that field is on the screen, for a chooser to open under it.
    place: function () { return current && current.isConnected && visible(current) ? current.getBoundingClientRect() : null; },
    stop: function () {
      stopped = true;
      listeners.forEach(function (l) { l[0].removeEventListener(l[1], l[2], l[3]); });
      if (button && button.host.isConnected) button.host.remove();
      button = null;
    }
  };
}

// Which of a sent form's typed passwords ({value, kind}, in the document's order) is the one to
// keep; null among none. One marked as new (autocomplete=new-password) is. With none marked and
// several typed, the form is a sign-up or a change of password whose fields say nothing of
// themselves: the new password is the one typed twice (itself and its confirmation), else the
// last that differs from the first (the current one, then the new) -- not the first, which on a
// change-password form is the old password, already kept. Outside the function above so that
// node's tests can ask it (test/forms.test.js).
SpatiKeyForms.passwordAmong = function (candidates) {
  'use strict';
  var i, j, first = candidates[0];
  if (!first) return null;
  for (i = 0; i < candidates.length; i++) if (candidates[i].kind === 'new-password') return candidates[i];
  var chosen = null;
  for (i = 0; i < candidates.length && !chosen; i++) {
    for (j = i + 1; j < candidates.length; j++) if (candidates[j].value === candidates[i].value) { chosen = candidates[i]; break; }
  }
  for (i = candidates.length - 1; i > 0 && !chosen; i--) if (candidates[i].value !== first.value) chosen = candidates[i];
  if (!chosen) return first;
  return { field: chosen.field, value: chosen.value, kind: 'new-password' };
};
