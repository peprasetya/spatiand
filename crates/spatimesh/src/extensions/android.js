// One extension in Android's WebView (docs/spatimesh.md, "Extensions"): put by the engine at the
// start of every frame's every page (the WebView's document-start scripts), in the page's own
// world -- the WebView has no other. The window makes one for each extension (set.rs,
// `page_start`), so that one that cannot run takes no other with it:
//
//   (THIS)(BOOT, EXTENSION, SECRET, ENTRY)
//
// BOOT is api.js's __spatimeshExtBoot. EXTENSION is {id, origin, content, page, scripts}: the
// API's configurations for a content script and for the extension's own pages, and each
// content script {runAt (0 start, 1 end, 2 idle), allFrames, mainWorld, include, globs, exclude
// (regular expressions' sources, as Script has them), css, js: a function of chrome, browser and
// globalThis that runs the entry's files (of nothing in the page's own world), or null}. The
// engine calls the top frame in by window[ENTRY], saying SECRET.
//
// The engine's message listener, `spatimeshExt`, carries the calls out (each frame its own) and
// deliveries back to frames other than the top one, where the engine cannot run code itself.
// Everything used after the page's own scripts may have run is taken now, before they can.
(function (boot, ext, secret, entry) {
  'use strict';
  var port = globalThis.spatimeshExt;
  if (!port || typeof port.postMessage !== 'function') return;
  var apply = Reflect.apply;
  var send = port.postMessage;
  var listen = port.addEventListener;
  var parse = JSON.parse;
  var RegExpC = RegExp;
  var test = RegExp.prototype.test;
  var defineProperty = Object.defineProperty;
  var CSSStyleSheetC = globalThis.CSSStyleSheet;
  var replaceSync = CSSStyleSheetC && CSSStyleSheetC.prototype.replaceSync;
  var setTimeoutF = setTimeout;
  var addEventListenerF = EventTarget.prototype.addEventListener;
  var documentN = document;
  var post = function (s) { apply(send, port, [s]); };
  var href = location.href;
  var own = location.origin === ext.origin;
  var top = window.top === window;

  // The extension's API in this frame, made when it is first wanted; and what code from the
  // window is given as `globalThis`.
  var made = null;
  var holder = null;
  function api() {
    if (!made) {
      made = boot(ext.content, post, true);
      holder = { __spatimeshExt: made };
    }
    return made;
  }

  function matches(source, url) { return apply(test, new RegExpC(source), [url]); }
  function wants(s, url) {
    return matches(s.include, url) && (s.globs === '' || matches(s.globs, url)) && !(s.exclude !== '' && matches(s.exclude, url));
  }

  function style(css) {
    if (!CSSStyleSheetC) return;
    try {
      var sheet = new CSSStyleSheetC();
      apply(replaceSync, sheet, [css]);
      documentN.adoptedStyleSheets = documentN.adoptedStyleSheets.concat([sheet]);
    } catch (e) {}
  }

  // At document_start now; at document_end once the page is parsed; at document_idle a moment
  // after that.
  function when(runAt, fn) {
    if (runAt === 0) return fn();
    var later = runAt === 2 ? function () { setTimeoutF(fn, 0); } : fn;
    if (documentN.readyState === 'loading') apply(addEventListenerF, documentN, ['DOMContentLoaded', later, { once: true }]);
    else later();
  }

  function run(s) {
    try {
      if (s.mainWorld) apply(s.js, window, []);
      else {
        var a = api();
        apply(s.js, window, [a.chrome, a.browser || undefined, a.global]);
      }
    } catch (e) {
      console.error(e);
    }
  }

  if (own) {
    // The extension's own page: its API as globals.
    boot(ext.page, post, false);
  } else if (/^(https?|file|ftp):/.test(href)) {
    ext.scripts.forEach(function (s) {
      if (!(top || s.allFrames) || !wants(s, href)) return;
      // Style sheets before anything is shown, whenever the scripts go, as in Chrome.
      if (s.css) style(s.css);
      if (s.js) when(s.runAt, function () { run(s); });
    });
  }

  // Code the window runs in the extension's name in the top frame (the engine's
  // evaluateJavascript): given the extension's API as `globalThis`, made first if `mayBoot`.
  if (top && !own) {
    defineProperty(window, entry, {
      value: function (said, fn, mayBoot) {
        if (said !== secret || typeof fn !== 'function' || (!made && !mayBoot)) return undefined;
        api();
        return fn(holder);
      },
      enumerable: false,
      configurable: false,
      writable: false
    });
  }

  // Deliveries for this frame's contexts of the extension, from the engine: {e: the extension,
  // d: what api.js is to be given}. Only the engine's own messages (trusted events) are taken.
  apply(listen, port, ['message', function (event) {
    if (!event.isTrusted) return;
    var m;
    try { m = parse(event.data); } catch (e) { return; }
    if (!m || m.e !== ext.id || typeof m.d !== 'string') return;
    var target = own ? globalThis.__spatimeshExt : made;
    if (target) target.deliver(m.d);
  }]);
})
