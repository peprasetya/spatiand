// SpatiMesh's media in the page: every <video> and <audio>, every MediaSource, every AudioContext
// played by Spatiand's own player (FFmpeg, with each platform's decoder) instead of the engine's,
// and the page none the wiser. Put into every frame at the start of every document, in the page's
// own world, before any of the page's scripts (docs/spatimesh.md, "Media").
//
// What the page sees is unchanged: HTMLMediaElement's properties, methods and events, TimeRanges,
// MediaError, play()'s promise, MediaSource and SourceBuffer, AudioContext's destination. Behind
// them, the element has no media of its own: its box is painted in the key colour, and SpatiMesh's
// window draws the film there, under the page, so that whatever the page draws over the film (its
// controls, its captions, a gradient) is still over it. The sound goes to the room, at the window.
//
// One frame speaks to the window: the top one. Others speak to their parent (postMessage), which
// adds where the frame is on its own page to what is said about places and passes it on up; what
// the window says comes down the same way. `send` is the engine's way out, given by the engine as
// `__spatimeshMediaPost` before this runs (top frame only); what comes in arrives at
// `__spatimeshMediaIn`. DRM is the engine's: a page that asks for a key system keeps the engine's
// own MediaSource from then on.
(function () {
  'use strict';
  if (window.__spatimeshMediaOn) return;
  try { Object.defineProperty(window, '__spatimeshMediaOn', { value: true }); } catch (e) { return; }
  if (typeof HTMLMediaElement === 'undefined') return;

  // ---- what the page could change later, kept as it is now ----
  var O = Object, defineProperty = O.defineProperty, getOwn = O.getOwnPropertyDescriptor;
  var JSONstringify = JSON.stringify, JSONparse = JSON.parse;
  var Promise_ = Promise, URL_ = URL, Event_ = Event, setTimeout_ = setTimeout, clearTimeout_ = clearTimeout;
  var raf = window.requestAnimationFrame ? window.requestAnimationFrame.bind(window) : function (f) { return setTimeout_(f, 16); };
  var now = performance.now.bind(performance);
  var MediaProto = HTMLMediaElement.prototype;
  var VideoProto = typeof HTMLVideoElement !== 'undefined' ? HTMLVideoElement.prototype : null;
  var ElementProto = Element.prototype;
  var nativeGetAttribute = ElementProto.getAttribute, nativeSetAttribute = ElementProto.setAttribute;
  var nativeRemoveAttribute = ElementProto.removeAttribute, nativeHasAttribute = ElementProto.hasAttribute;
  var nativeLoad = MediaProto.load, nativePlay = MediaProto.play, nativePause = MediaProto.pause;
  var nativeDispatch = EventTarget.prototype.dispatchEvent;
  var nativeAddListener = EventTarget.prototype.addEventListener;
  var nativeRemoveListener = EventTarget.prototype.removeEventListener;
  var nativeGetComputed = window.getComputedStyle.bind(window);
  var nativeRect = ElementProto.getBoundingClientRect;
  var postMessage_ = function (w, data) { try { w.postMessage(data, '*'); } catch (e) {} };
  function native(proto, name) {
    for (var p = proto; p; p = O.getPrototypeOf(p)) { var d = getOwn(p, name); if (d) return d; }
    return null;
  }
  var N = {};
  ['src', 'currentSrc', 'srcObject', 'currentTime', 'duration', 'paused', 'ended', 'volume', 'muted', 'playbackRate',
    'defaultPlaybackRate', 'readyState', 'networkState', 'buffered', 'seekable', 'played', 'seeking', 'error', 'controls',
    'loop', 'autoplay', 'preload', 'defaultMuted', 'preservesPitch'].forEach(function (k) { N[k] = native(MediaProto, k); });
  if (VideoProto) ['videoWidth', 'videoHeight', 'poster'].forEach(function (k) { N[k] = native(VideoProto, k); });

  var TOP = window.top === window;
  var KEY = 'rgb(1, 254, 3)';
  var frameKey = Math.random().toString(36).slice(2) + Date.now().toString(36);
  var send;           // to the window (top) or the parent (others)
  var outbox = [];    // said before the way out was known

  // ---- the way out, and in ----
  if (TOP) {
    var post = window.__spatimeshMediaPost;
    try { delete window.__spatimeshMediaPost; } catch (e) {}
    send = function (m) {
      m.f = m.f || frameKey;
      var s = JSONstringify(m);
      if (post) { try { post(s); } catch (e) {} } else outbox.push(s);
    };
    defineProperty(window, '__spatimeshMediaIn', {
      value: function (json) { var m = typeof json === 'string' ? JSONparse(json) : json; down(m); },
      configurable: false, writable: false, enumerable: false
    });
    defineProperty(window, '__spatimeshMediaSetPost', {
      value: function (p) { if (post) return; post = p; var q = outbox; outbox = []; q.forEach(function (s) { try { post(s); } catch (e) {} }); },
      configurable: false, writable: false, enumerable: false
    });
  } else {
    send = function (m) { m.f = m.f || frameKey; postMessage_(window.parent, { __spatimeshMediaUp: m }); };
  }

  // Frames below this one, by their key: where to pass what comes down for them.
  var below = {};
  function down(m) {
    if (!m || typeof m !== 'object') return;
    if (m.f === frameKey || !m.f) { receive(m); return; }
    var w = below[m.f];
    if (w) postMessage_(w, { __spatimeshMediaDown: m });
  }

  // The frame element a window is shown in, on this page.
  function frameOf(source) {
    var frames = document.querySelectorAll('iframe,frame');
    for (var i = 0; i < frames.length; i++) if (frames[i].contentWindow === source) return frames[i];
    return null;
  }

  nativeAddListener.call(window, 'message', function (ev) {
    var d = ev.data;
    if (!d || typeof d !== 'object') return;
    if (d.__spatimeshMediaUp && ev.source && ev.source !== window) {
      ev.stopImmediatePropagation();
      var m = d.__spatimeshMediaUp;
      below[m.f] = ev.source;
      if (m.t === 'fullscreen') {
        // A frame below going fullscreen: its frame element fills this page, as browsers do.
        var frame = frameOf(ev.source);
        if (frame) { if (m.on) enterFs(frame, ev.source); else if (fsElement === frame) leaveFs(false, true); }
        return;
      }
      // Places said by a frame below are on its page: here they are moved to this one.
      if (m.places) {
        var el = frameOf(ev.source);
        if (!el) return;
        var r = nativeRect.call(el), cs = nativeGetComputed(el);
        var dx = r.left + (parseFloat(cs.borderLeftWidth) || 0) + (parseFloat(cs.paddingLeft) || 0);
        var dy = r.top + (parseFloat(cs.borderTopWidth) || 0) + (parseFloat(cs.paddingTop) || 0);
        var shown = r.width > 0 && r.height > 0 && cs.visibility !== 'hidden';
        m.places.forEach(function (p) { p.x += dx; p.y += dy; if (!shown) p.shown = false; });
      }
      send(m);
    } else if (d.__spatimeshMediaDown && ev.source === window.parent) {
      ev.stopImmediatePropagation();
      down(d.__spatimeshMediaDown);
    }
  }, true);

  // ---- events of our own, and the engine's swallowed ----
  var ours = new WeakSet();
  var managed = new WeakSet();
  var MEDIA_EVENTS = ['loadstart', 'progress', 'suspend', 'abort', 'error', 'emptied', 'stalled', 'loadedmetadata', 'loadeddata',
    'canplay', 'canplaythrough', 'playing', 'waiting', 'seeking', 'seeked', 'ended', 'durationchange', 'timeupdate', 'play', 'pause',
    'ratechange', 'resize', 'volumechange', 'encrypted', 'waitingforkey'];
  MEDIA_EVENTS.forEach(function (name) {
    nativeAddListener.call(window, name, function (ev) {
      if (managed.has(ev.target) && !ours.has(ev)) ev.stopImmediatePropagation();
    }, true);
  });
  function fire(el, name) {
    var ev = new Event_(name);
    ours.add(ev);
    nativeDispatch.call(el, ev);
  }
  function soon(f) { Promise_.resolve().then(f); }

  // A user has done something on this page: what lets a sound start by itself, as browsers do.
  var activated = false;
  ['pointerdown', 'mousedown', 'keydown', 'touchstart'].forEach(function (name) {
    nativeAddListener.call(window, name, function (ev) { if (ev.isTrusted) activated = true; }, true);
  });
  function mayStart(st) {
    if (st.muted || st.volume === 0) return true;
    if (activated) return true;
    try { if (navigator.userActivation && navigator.userActivation.hasBeenActive) return true; } catch (e) {}
    return false;
  }

  // ---- TimeRanges and MediaError as the page knows them ----
  function ranges(list) {
    var r = O.create(typeof TimeRanges !== 'undefined' ? TimeRanges.prototype : O.prototype);
    defineProperty(r, 'length', { value: list.length });
    defineProperty(r, 'start', { value: function (i) { if (i < 0 || i >= list.length) throw new DOMException('Index out of range', 'IndexSizeError'); return list[i][0]; } });
    defineProperty(r, 'end', { value: function (i) { if (i < 0 || i >= list.length) throw new DOMException('Index out of range', 'IndexSizeError'); return list[i][1]; } });
    return r;
  }
  function mediaError(code, message) {
    var e = O.create(typeof MediaError !== 'undefined' ? MediaError.prototype : O.prototype);
    defineProperty(e, 'code', { value: code });
    defineProperty(e, 'message', { value: message || '' });
    return e;
  }

  // ---- each element's state ----
  var nextId = 1;
  var byId = {};
  var states = new WeakMap();
  // Object URLs made for this page's MediaSources (step 4), by URL.
  var sources = {};

  function stateOf(el) {
    var st = states.get(el);
    if (st) return st;
    st = {
      id: nextId++, el: el, url: '', opened: false, at: 0, base: 0, since: now(), duration: NaN, paused: true, ended: false,
      seeking: false, waiting: false, readyState: 0, networkState: 0, width: 0, height: 0, buffered: [], seekable: [], played: [],
      volume: 1, muted: nativeHasAttribute.call(el, 'muted'), rate: 1, defaultRate: 1, error: null, poster: null, controls: false,
      playWaiters: [], shown: null, posterHidden: false, sized: false, wantFrames: 0, frame: null, source: null
    };
    states.set(el, st);
    byId[st.id] = st;
    managed.add(el);
    // What the engine was given in the page's markup is ours now, and the engine has nothing.
    take(el, st);
    return st;
  }

  function isVideo(el) { return VideoProto && el instanceof HTMLVideoElement; }

  function take(el, st) {
    var src = nativeGetAttribute.call(el, 'src');
    if (src !== null) { st.attr = src; nativeRemoveAttribute.call(el, 'src'); }
    if (isVideo(el)) {
      if (nativeHasAttribute.call(el, 'controls')) { st.controls = true; nativeRemoveAttribute.call(el, 'controls'); }
      el.style.setProperty('background-color', KEY, 'important');
    } else if (nativeHasAttribute.call(el, 'controls')) {
      st.controls = true;
    }
    var kids = el.querySelectorAll('source');
    for (var i = 0; i < kids.length; i++) takeSource(kids[i]);
    try { nativeLoad.call(el); } catch (e) {}
  }

  var sourceSrc = new WeakMap();
  function takeSource(s) {
    var src = nativeGetAttribute.call(s, 'src');
    if (src !== null) { sourceSrc.set(s, src); nativeRemoveAttribute.call(s, 'src'); }
  }

  function absolute(url) {
    try { return new URL_(url, document.baseURI).href; } catch (e) { return url; }
  }

  // The address an element is to play: its src, or its first <source>'s.
  function chosen(el, st) {
    if (st.src !== undefined && st.src !== null && st.src !== '') return absolute(st.src);
    if (st.attr) return absolute(st.attr);
    var kids = el.querySelectorAll('source');
    for (var i = 0; i < kids.length; i++) {
      var s = sourceSrc.has(kids[i]) ? sourceSrc.get(kids[i]) : nativeGetAttribute.call(kids[i], 'src');
      if (s) return absolute(s);
    }
    return '';
  }

  function time(st) {
    if (st.paused || st.seeking || st.waiting || st.ended || st.readyState < 3) return st.at;
    var t = st.base + (now() - st.since) / 1000 * st.rate;
    if (!isNaN(st.duration) && st.duration !== Infinity) t = Math.min(t, st.duration);
    return t;
  }

  // ---- loading ----
  function load(el, st, why) {
    var url = chosen(el, st);
    var had = st.opened;
    if (had) { send({ t: 'close', id: st.id }); st.opened = false; }
    st.playWaiters.splice(0).forEach(function (w) { w.reject(new DOMException('The play() request was interrupted by a new load request.', 'AbortError')); });
    if (st.readyState > 0 || had) {
      st.readyState = 0; st.duration = NaN; st.buffered = []; st.seekable = []; st.at = 0; st.base = 0; st.ended = false;
      if (!st.paused) { st.paused = true; fire(el, 'pause'); }
      fire(el, 'emptied');
    }
    st.error = null;
    st.width = 0; st.height = 0;
    showPoster(st, true);
    st.url = url;
    if (!url) { st.networkState = 3; return; }
    if (st.source) st.source.detach();
    st.source = null;
    var ms = sources[url];
    if (ms && ms.__spatimesh) { st.source = ms.__spatimesh; }
    st.networkState = 2;
    fire(el, 'loadstart');
    var preload = (nativeGetAttribute.call(el, 'preload') || 'auto').toLowerCase();
    if (preload === 'none' && !st.wantPlay && !nativeHasAttribute.call(el, 'autoplay')) { st.networkState = 1; fire(el, 'suspend'); return; }
    open(el, st);
  }

  function open(el, st) {
    if (st.opened || !st.url) return;
    st.opened = true;
    var m = { t: 'open', id: st.id, url: st.url, page: location.href, muted: st.muted, volume: st.volume, rate: st.rate,
      loop: nativeHasAttribute.call(el, 'loop'), video: !!isVideo(el), at: st.at };
    if (st.source) { m.url = ''; m.source = st.source.id; st.source.attach(st); }
    send(m);
    places();
  }

  // ---- what the window says ----
  function receive(m) {
    if (m.t === 'ms') { msReceive(m); return; }
    if (m.t === 'fullscreen-exit') { if (fsElement) leaveFs(); return; }
    if (m.t === 'audio') { audioReceive(m); return; }
    var st = byId[m.id];
    if (!st) return;
    var el = st.el;
    if (m.t === 'release') {
      // Spatiand's player could not open it: the engine plays it, as it would have.
      var url = st.url, wanted = st.wantPlay;
      release(el, st);
      if (url && N.src && N.src.set) N.src.set.call(el, url);
      if (wanted) { var pr = nativePlay.call(el); if (pr && pr.catch) pr.catch(function () {}); }
      return;
    }
    if (m.t === 'control') {
      // SpatiMesh's own controls for the film, done through the element as the page's would be.
      if (m.do === 'play') { var q = play(el, st); if (q && q.catch) q.catch(function () {}); }
      else if (m.do === 'pause') pause(el, st);
      else if (m.do === 'mute') { st.muted = !st.muted; send({ t: 'volume', id: st.id, volume: st.volume, muted: st.muted }); fire(el, 'volumechange'); }
      else if (m.do === 'seekTo') seekTo(st, m.at);
      return;
    }
    if (m.t === 'meta') {
      var first = st.readyState < 1;
      var duration = m.duration === null ? NaN : (m.duration < 0 ? Infinity : m.duration);
      var durationChanged = st.duration !== duration && !(isNaN(st.duration) && isNaN(duration));
      st.duration = duration;
      st.width = m.width || 0; st.height = m.height || 0;
      st.fps = m.fps > 0 ? m.fps : 30;
      st.seekable = st.duration > 0 && st.duration !== Infinity ? [[0, st.duration]] : [];
      if (first) {
        st.readyState = 1;
        if (durationChanged) fire(el, 'durationchange');
        if (st.width) fire(el, 'resize');
        fire(el, 'loadedmetadata');
        size(st);
        if (st.pendingSeek !== undefined) { var to = st.pendingSeek; st.pendingSeek = undefined; seekTo(st, to); }
      } else {
        if (durationChanged) fire(el, 'durationchange');
        if (m.resized) fire(el, 'resize');
      }
    } else if (m.t === 'state') {
      var before = { paused: st.paused, seeking: st.seeking, waiting: st.waiting, ended: st.ended, rs: st.readyState };
      if (typeof m.at === 'number') { st.at = m.at; st.base = m.at; st.since = now(); }
      if (m.buffered) st.buffered = m.buffered;
      if (m.ready && st.readyState < 4) {
        var from = st.readyState;
        st.readyState = 4;
        st.networkState = 1;
        if (from < 2) fire(el, 'loadeddata');
        if (from < 3) fire(el, 'canplay');
        fire(el, 'canplaythrough');
        if (!st.paused && !st.waiting) fire(el, 'playing');
        if (nativeHasAttribute.call(el, 'autoplay') && st.paused && !st.autoplayed) {
          st.autoplayed = true;
          if (mayStart(st)) play(el, st);
        }
      }
      if (m.shown) showPoster(st, false);
      if (typeof m.waiting === 'boolean' && m.waiting !== st.waiting) {
        st.waiting = m.waiting;
        if (m.waiting && !st.paused) fire(el, 'waiting');
        else if (!m.waiting && !st.paused) fire(el, 'playing');
      }
      if (m.seeked && st.seeking) {
        st.seeking = false;
        var done = function () { fire(el, 'timeupdate'); fire(el, 'seeked'); };
        if (st.wantFrames && m.frame) frameIn(st, m.frame, done); else done();
        return;
      }
      if (m.frame && st.wantFrames) frameIn(st, m.frame, null);
      if (m.ended && !st.ended) {
        st.ended = true;
        st.at = st.duration > 0 ? st.duration : st.at; st.base = st.at;
        fire(el, 'timeupdate');
        if (!st.paused) { st.paused = true; fire(el, 'pause'); }
        fire(el, 'ended');
        return;
      }
      if (m.tick && !st.paused && !st.seeking) fire(el, 'timeupdate');
      if (m.progress) fire(el, 'progress');
    } else if (m.t === 'error') {
      st.error = mediaError(m.code || 4, m.message);
      st.networkState = 3;
      st.playWaiters.splice(0).forEach(function (w) { w.reject(new DOMException(m.message || 'Failed to load because no supported source was found.', 'NotSupportedError')); });
      fire(el, 'error');
    } else if (m.t === 'frame') {
      frameIn(st, m.frame, null);
    }
  }

  // The poster is the engine's to draw until there is a picture of the film; then it goes, and
  // the box is the key alone.
  function showPoster(st, show) {
    if (!isVideo(st.el)) return;
    if (!show && !st.posterHidden) {
      st.poster = nativeGetAttribute.call(st.el, 'poster');
      if (st.poster !== null) nativeRemoveAttribute.call(st.el, 'poster');
      st.posterHidden = true;
    } else if (show && st.posterHidden) {
      if (st.poster !== null) nativeSetAttribute.call(st.el, 'poster', st.poster);
      st.posterHidden = false;
    }
  }

  // An element with no size of its own takes the film's, as it would with the engine's media.
  function size(st) {
    var el = st.el;
    if (!isVideo(el) || !st.width || !st.height) return;
    if (nativeHasAttribute.call(el, 'width') || nativeHasAttribute.call(el, 'height')) return;
    if (el.style.width || el.style.height) return;
    var cs = nativeGetComputed(el);
    if (cs.width === '300px' && cs.height === '150px') {
      el.style.setProperty('aspect-ratio', st.width + ' / ' + st.height);
      el.style.setProperty('width', st.width + 'px');
      st.sized = true;
    }
  }

  // ---- playing ----
  function play(el, st) {
    return new Promise_(function (resolve, reject) {
      if (st.error) { reject(new DOMException('The element has no supported sources.', 'NotSupportedError')); return; }
      if (!mayStart(st)) { reject(new DOMException("play() failed because the user didn't interact with the document first.", 'NotAllowedError')); return; }
      st.wantPlay = true;
      if (!st.url && !st.opened) { load(el, st, 'play'); }
      if (!st.opened) open(el, st);
      if (st.ended) { st.ended = false; seekTo(st, 0); }
      if (st.paused) {
        st.paused = false;
        st.base = st.at; st.since = now();
        fire(el, 'play');
        send({ t: 'play', id: st.id });
        if (st.readyState >= 3 && !st.waiting) { soon(function () { fire(el, 'playing'); }); resolve(); return; }
        else fire(el, 'waiting');
      } else if (st.readyState >= 3) { resolve(); return; }
      st.playWaiters.push({ resolve: resolve, reject: reject });
    });
  }

  function pause(el, st) {
    st.wantPlay = false;
    if (!st.opened && st.url) open(el, st);
    if (!st.paused) {
      st.at = time(st); st.base = st.at; st.since = now();
      st.paused = true;
      send({ t: 'pause', id: st.id, at: st.at });
      fire(el, 'timeupdate');
      fire(el, 'pause');
      st.playWaiters.splice(0).forEach(function (w) { w.reject(new DOMException('The play() request was interrupted by a call to pause().', 'AbortError')); });
    }
  }

  function seekTo(st, t) {
    if (!isFinite(t)) return;
    if (st.duration > 0 && st.duration !== Infinity) t = Math.min(Math.max(0, t), st.duration);
    else t = Math.max(0, t);
    st.at = t; st.base = t; st.since = now(); st.ended = false;
    if (st.readyState < 1) { st.pendingSeek = t; return; }
    st.seeking = true;
    fire(st.el, 'seeking');
    if (likelyDrawn(st) && !st.wantFrames) st.wantFrames = now();
    send({ t: 'seek', id: st.id, at: t, picture: !!st.wantFrames || likelyDrawn(st) });
  }

  function settleWaiters(st) {
    if (!st.paused && st.readyState >= 3 && !st.waiting) {
      st.playWaiters.splice(0).forEach(function (w) { w.resolve(); });
    }
  }

  // ---- the element as the page sees it ----
  function accessor(proto, name, get, set) {
    var nd = N[name] || native(proto, name);
    defineProperty(proto, name, {
      configurable: true, enumerable: true,
      get: function () { return isOurs(this) ? get.call(this, stateOf(this)) : nd && nd.get ? nd.get.call(this) : undefined; },
      set: set ? function (v) { if (isOurs(this)) set.call(this, stateOf(this), v); else if (nd && nd.set) nd.set.call(this, v); } : undefined
    });
  }
  // An element is ours unless its media is something only the engine can play: a camera's
  // stream, or a MediaSource of the engine's own (a page with DRM).
  var engines = new WeakSet();
  function isOurs(el) { return !engines.has(el); }

  accessor(MediaProto, 'src', function (st) { return st.src !== undefined ? (st.src ? absolute(st.src) : '') : (st.attr ? absolute(st.attr) : ''); },
    function (st, v) {
      v = String(v);
      if (/^blob:/.test(v) && sources[v] && !sources[v].__spatimesh) { release(this, st); nd('src').set.call(this, v); return; }
      st.src = v; st.attr = v; load(this, st, 'src');
    });
  function nd(name) { return N[name]; }
  accessor(MediaProto, 'currentSrc', function (st) { return st.url || ''; });
  accessor(MediaProto, 'srcObject', function (st) { return st.object || null; }, function (st, v) {
    // A MediaStream (a camera, a call) is the engine's; a MediaSource given directly is ours.
    if (v && typeof MediaStream !== 'undefined' && v instanceof MediaStream) { release(this, st); N.srcObject.set.call(this, v); return; }
    st.object = v;
    if (v && v.__spatimesh) { st.src = ''; st.url = ''; st.source = v.__spatimesh; open(this, st); }
  });
  accessor(MediaProto, 'currentTime', function (st) { return time(st); }, function (st, v) { seekTo(st, Number(v)); });
  accessor(MediaProto, 'duration', function (st) { return st.duration; });
  accessor(MediaProto, 'paused', function (st) { return st.paused; });
  accessor(MediaProto, 'ended', function (st) { return st.ended; });
  accessor(MediaProto, 'seeking', function (st) { return st.seeking; });
  accessor(MediaProto, 'readyState', function (st) { return st.readyState; });
  accessor(MediaProto, 'networkState', function (st) { return st.networkState; });
  accessor(MediaProto, 'error', function (st) { return st.error; });
  accessor(MediaProto, 'buffered', function (st) { return ranges(st.buffered); });
  accessor(MediaProto, 'seekable', function (st) { return ranges(st.seekable); });
  accessor(MediaProto, 'played', function (st) { return ranges(st.played); });
  accessor(MediaProto, 'volume', function (st) { return st.volume; }, function (st, v) {
    v = Number(v);
    if (!(v >= 0 && v <= 1)) throw new DOMException("The volume provided (" + v + ") is outside the range [0, 1].", 'IndexSizeError');
    if (v === st.volume) return;
    st.volume = v; send({ t: 'volume', id: st.id, volume: st.volume, muted: st.muted }); fire(this, 'volumechange');
  });
  accessor(MediaProto, 'muted', function (st) { return st.muted; }, function (st, v) {
    v = !!v; if (v === st.muted) return;
    st.muted = v; send({ t: 'volume', id: st.id, volume: st.volume, muted: st.muted }); fire(this, 'volumechange');
  });
  accessor(MediaProto, 'playbackRate', function (st) { return st.rate; }, function (st, v) {
    v = Number(v); if (!isFinite(v)) throw new TypeError('The provided double value is non-finite.');
    if (v === st.rate) return;
    st.at = time(st); st.base = st.at; st.since = now();
    st.rate = v; send({ t: 'rate', id: st.id, rate: v }); fire(this, 'ratechange');
  });
  accessor(MediaProto, 'defaultPlaybackRate', function (st) { return st.defaultRate; }, function (st, v) { st.defaultRate = Number(v); fire(this, 'ratechange'); });
  accessor(MediaProto, 'controls', function (st) { return st.controls; }, function (st, v) {
    st.controls = !!v;
    if (!isVideo(this)) { if (v) nativeSetAttribute.call(this, 'controls', ''); else nativeRemoveAttribute.call(this, 'controls'); }
    places();
  });
  if (VideoProto) {
    accessor(VideoProto, 'videoWidth', function (st) { return st.width; });
    accessor(VideoProto, 'videoHeight', function (st) { return st.height; });
    accessor(VideoProto, 'poster', function (st) {
      var p = st.posterHidden ? st.poster : nativeGetAttribute.call(this, 'poster');
      return p ? absolute(p) : '';
    }, function (st, v) { if (st.posterHidden) st.poster = String(v); else nativeSetAttribute.call(this, 'poster', String(v)); });
  }

  defineProperty(MediaProto, 'load', { configurable: true, writable: true, value: function () {
    if (!isOurs(this)) return nativeLoad.call(this);
    var st = stateOf(this); load(this, st, 'load');
  } });
  defineProperty(MediaProto, 'play', { configurable: true, writable: true, value: function () {
    if (!isOurs(this)) return nativePlay.call(this);
    return play(this, stateOf(this));
  } });
  defineProperty(MediaProto, 'pause', { configurable: true, writable: true, value: function () {
    if (!isOurs(this)) return nativePause.call(this);
    pause(this, stateOf(this));
  } });
  defineProperty(MediaProto, 'fastSeek', { configurable: true, writable: true, value: function (t) {
    if (isOurs(this)) seekTo(stateOf(this), Number(t));
  } });
  var nativeCanPlay = MediaProto.canPlayType;
  defineProperty(MediaProto, 'canPlayType', { configurable: true, writable: true, value: function (type) {
    return canPlay(String(type)) || nativeCanPlay.call(this, type);
  } });
  if (VideoProto) {
    defineProperty(VideoProto, 'getVideoPlaybackQuality', { configurable: true, writable: true, value: function () {
      var st = stateOf(this);
      return { creationTime: now(), totalVideoFrames: Math.round(time(st) * 30), droppedVideoFrames: 0, corruptedVideoFrames: 0 };
    } });
  }

  // What Spatiand's FFmpeg plays: nearly everything. A codec it cannot play still plays its
  // other track, so "maybe" for anything in a container it knows.
  function canPlay(type) {
    var t = type.toLowerCase().split(';')[0].trim();
    if (!/^(video|audio|application)\//.test(t)) return '';
    var known = /^(video|audio)\/(mp4|webm|ogg|x-matroska|matroska|mkv|quicktime|x-msvideo|avi|msvideo|mpeg|mp2t|mp3|mpeg3|aac|aacp|flac|x-flac|wav|wave|x-wav|opus|3gpp|3gpp2|x-ms-wmv|x-ms-asf|x-flv|mp4a-latm|x-m4a|m4a|ac3|eac3|x-aiff|aiff|amr|vnd\.dlna\.mpeg-tts)$/.test(t)
      || t === 'application/ogg' || t === 'application/vnd.apple.mpegurl' || t === 'application/x-mpegurl';
    return known ? (/codecs=/.test(type) ? 'probably' : 'maybe') : '';
  }

  // Attributes the page reads back as it wrote them, though the engine no longer has them.
  defineProperty(ElementProto, 'getAttribute', { configurable: true, writable: true, value: function (name) {
    if (managed.has(this) && typeof name === 'string') {
      var n = name.toLowerCase(), st = states.get(this);
      if (n === 'src') return st.src !== undefined ? st.src : (st.attr !== undefined ? st.attr : null);
      if (n === 'controls' && isVideo(this)) return st.controls ? '' : null;
      if (n === 'poster' && st.posterHidden) return st.poster;
    } else if (sourceSrc.has(this) && typeof name === 'string' && name.toLowerCase() === 'src') return sourceSrc.get(this);
    return nativeGetAttribute.call(this, name);
  } });
  defineProperty(ElementProto, 'hasAttribute', { configurable: true, writable: true, value: function (name) {
    if (managed.has(this) && typeof name === 'string') {
      var n = name.toLowerCase(), st = states.get(this);
      if (n === 'src') return (st.src !== undefined ? st.src : st.attr) != null;
      if (n === 'controls' && isVideo(this)) return st.controls;
      if (n === 'poster' && st.posterHidden) return st.poster !== null;
    } else if (sourceSrc.has(this) && typeof name === 'string' && name.toLowerCase() === 'src') return true;
    return nativeHasAttribute.call(this, name);
  } });
  defineProperty(ElementProto, 'setAttribute', { configurable: true, writable: true, value: function (name, value) {
    var n = typeof name === 'string' ? name.toLowerCase() : '';
    if (this instanceof HTMLMediaElement && isOurs(this)) {
      if (n === 'src') { var st = stateOf(this); st.src = String(value); st.attr = st.src; load(this, st, 'attr'); return; }
      if (n === 'controls' && isVideo(this)) { stateOf(this).controls = true; places(); return; }
      if (n === 'poster' && states.has(this) && states.get(this).posterHidden) { states.get(this).poster = String(value); return; }
    }
    if (typeof HTMLSourceElement !== 'undefined' && this instanceof HTMLSourceElement && n === 'src') { sourceSrc.set(this, String(value)); return; }
    return nativeSetAttribute.call(this, name, value);
  } });
  defineProperty(ElementProto, 'removeAttribute', { configurable: true, writable: true, value: function (name) {
    var n = typeof name === 'string' ? name.toLowerCase() : '';
    if (managed.has(this)) {
      var st = states.get(this);
      if (n === 'src') { st.src = undefined; st.attr = undefined; return; }
      if (n === 'controls' && isVideo(this)) { st.controls = false; places(); return; }
      if (n === 'poster' && st.posterHidden) { st.poster = null; return; }
    }
    if (sourceSrc.has(this) && n === 'src') { sourceSrc.delete(this); return; }
    return nativeRemoveAttribute.call(this, name);
  } });
  if (typeof HTMLSourceElement !== 'undefined') {
    var srcDesc = native(HTMLSourceElement.prototype, 'src');
    defineProperty(HTMLSourceElement.prototype, 'src', { configurable: true, enumerable: true,
      get: function () { return sourceSrc.has(this) ? absolute(sourceSrc.get(this)) : srcDesc.get.call(this); },
      set: function (v) { sourceSrc.set(this, String(v)); } });
  }

  // Hand an element back to the engine: what it plays only the engine can.
  function release(el, st) {
    engines.add(el);
    managed.delete(el);
    if (st.opened) send({ t: 'close', id: st.id });
    st.opened = false;
    el.style.removeProperty('background-color');
    if (st.sized) { el.style.removeProperty('aspect-ratio'); el.style.removeProperty('width'); }
    if (st.controls && isVideo(el)) nativeSetAttribute.call(el, 'controls', '');
    showPoster(st, true);
    delete byId[st.id];
    places();
  }

  // ---- elements as the parser makes them ----
  function adopt(node) {
    if (node instanceof HTMLMediaElement) {
      if (!states.has(node) && isOurs(node)) {
        var st = stateOf(node);
        if (chosen(node, st)) load(node, st, 'parsed');
      }
    } else if (typeof HTMLSourceElement !== 'undefined' && node instanceof HTMLSourceElement) {
      takeSource(node);
      var p = node.parentNode;
      if (p instanceof HTMLMediaElement && isOurs(p)) {
        var ps = stateOf(p);
        if (!ps.url && !ps.src && !ps.attr) load(p, ps, 'source');
      }
    }
    if (node.querySelectorAll) {
      var inner = node.querySelectorAll('video,audio,source');
      for (var i = 0; i < inner.length; i++) adopt(inner[i]);
    }
  }
  new MutationObserver(function (records) {
    records.forEach(function (r) {
      if (r.type === 'childList') for (var i = 0; i < r.addedNodes.length; i++) { if (r.addedNodes[i].nodeType === 1) adopt(r.addedNodes[i]); }
      else if (r.type === 'attributes' && r.target instanceof HTMLMediaElement && isOurs(r.target)) {
        var st = stateOf(r.target), v = nativeGetAttribute.call(r.target, r.attributeName);
        if (r.attributeName === 'src' && v !== null) { nativeRemoveAttribute.call(r.target, 'src'); st.src = v; st.attr = v; load(r.target, st, 'attr'); }
        if (r.attributeName === 'controls' && v !== null && isVideo(r.target)) { nativeRemoveAttribute.call(r.target, 'controls'); st.controls = true; places(); }
      }
    });
  }).observe(document, { childList: true, subtree: true, attributes: true, attributeFilter: ['src', 'controls'] });

  // An element nobody keeps any more stops playing: the window is told when it leaves the page
  // and is not playing, or when it is collected.
  var gone = typeof FinalizationRegistry !== 'undefined' ? new FinalizationRegistry(function (id) { send({ t: 'close', id: id }); }) : null;

  // ---- where the films are on the page ----
  var placesDue = false;
  var lastPlaces = '';
  function places() { placesDue = true; }
  function look() {
    raf(look);
    var list = [];
    var any = false;
    for (var id in byId) {
      var st = byId[id], el = st.el;
      if (!st.opened) continue;
      any = true;
      if (isVideo(el) && el.style.getPropertyValue('background-color') !== KEY) el.style.setProperty('background-color', KEY, 'important');
      var shown = el.isConnected;
      var r = shown ? nativeRect.call(el) : null;
      var cs = shown ? nativeGetComputed(el) : null;
      if (cs && (cs.display === 'none' || cs.visibility === 'hidden')) shown = false;
      if (r && (r.width <= 0 || r.height <= 0)) shown = false;
      if (!isVideo(el) && !st.controls) shown = false;
      if (!shown) { list.push({ id: st.id, shown: false }); continue; }
      var bl = parseFloat(cs.borderLeftWidth) || 0, bt = parseFloat(cs.borderTopWidth) || 0;
      var pl = parseFloat(cs.paddingLeft) || 0, pt = parseFloat(cs.paddingTop) || 0;
      list.push({
        id: st.id, shown: true, video: !!isVideo(el),
        x: r.left + bl + pl, y: r.top + bt + pt,
        w: el.clientWidth - pl - (parseFloat(cs.paddingRight) || 0), h: el.clientHeight - pt - (parseFloat(cs.paddingBottom) || 0),
        fit: cs.objectFit || 'contain', pos: cs.objectPosition || '50% 50%', controls: st.controls
      });
    }
    if (!any && !placesDue) return;
    var s = JSONstringify(list);
    if (s !== lastPlaces || placesDue) {
      lastPlaces = s; placesDue = false;
      send({ t: 'places', places: list, zoom: window.devicePixelRatio || 1 });
    }
  }
  raf(look);

  // Whatever has not settled, settled: play()'s promises once playing, by the window's word.
  var settleTimer = function () {
    for (var id in byId) settleWaiters(byId[id]);
    setTimeout_(settleTimer, 30);
  };
  setTimeout_(settleTimer, 30);

  // ---- step 3: the film's picture, for a page that draws it itself ----
  // The engine has no picture of the film to give drawImage, WebGL, createImageBitmap,
  // VideoFrame or captureStream, so each film keeps a canvas of its own with its latest picture
  // in it, the film's own size, and that canvas is given them instead. The window sends pictures
  // only while a page is using them (asked again each second they are) -- and for a film the page
  // keeps out of sight or asks for with crossOrigin, as the ones a page makes thumbnails from are,
  // with every seek; `seeked` waits for that picture, so a thumbnail drawn on `seeked` has it.
  var nativeCanvasDraw = typeof CanvasRenderingContext2D !== 'undefined' ? CanvasRenderingContext2D.prototype.drawImage : null;
  function canvasOf(st) {
    if (!st.canvas) {
      st.canvas = document.createElement('canvas');
      st.canvas.width = st.width || 2; st.canvas.height = st.height || 2;
    }
    return st.canvas;
  }
  function frameIn(st, frame, then) {
    var bytes;
    try {
      var text = atob(frame);
      bytes = new Uint8Array(text.length);
      for (var i = 0; i < text.length; i++) bytes[i] = text.charCodeAt(i);
    } catch (e) { if (then) then(); return; }
    createImageBitmap(new Blob([bytes], { type: 'image/jpeg' })).then(function (bitmap) {
      var c = canvasOf(st);
      var w = st.width || bitmap.width, h = st.height || bitmap.height;
      if (c.width !== w || c.height !== h) { c.width = w; c.height = h; }
      nativeCanvasDraw.call(c.getContext('2d'), bitmap, 0, 0, w, h);
      if (bitmap.close) bitmap.close();
      st.hasFrame = true;
      if (then) then();
    }, function () { if (then) then(); });
  }
  // A page has used the film's picture: the window keeps sending them.
  function used(st) {
    var t = now();
    if (!st.wantFrames || t - st.wantFrames > 1000) { st.wantFrames = t; send({ t: 'frames', id: st.id }); }
    return canvasOf(st);
  }
  // Films a page likely makes pictures of: asked for with crossOrigin, or kept out of sight.
  function likelyDrawn(st) {
    var el = st.el;
    if (nativeHasAttribute.call(el, 'crossorigin')) return true;
    if (!el.isConnected) return true;
    var cs = nativeGetComputed(el);
    return cs.display === 'none' || cs.visibility === 'hidden';
  }
  function swap(source) {
    if (source && VideoProto && source instanceof HTMLVideoElement && managed.has(source)) return used(stateOf(source));
    return source;
  }
  if (nativeCanvasDraw) {
    defineProperty(CanvasRenderingContext2D.prototype, 'drawImage', { configurable: true, writable: true, value: function (image) {
      if (image && managed.has(image)) {
        var args = Array.prototype.slice.call(arguments);
        args[0] = swap(image);
        return nativeCanvasDraw.apply(this, args);
      }
      return nativeCanvasDraw.apply(this, arguments);
    } });
  }
  if (typeof OffscreenCanvasRenderingContext2D !== 'undefined') {
    var offDraw = OffscreenCanvasRenderingContext2D.prototype.drawImage;
    defineProperty(OffscreenCanvasRenderingContext2D.prototype, 'drawImage', { configurable: true, writable: true, value: function (image) {
      var args = Array.prototype.slice.call(arguments);
      args[0] = swap(image);
      return offDraw.apply(this, args);
    } });
  }
  ['WebGLRenderingContext', 'WebGL2RenderingContext'].forEach(function (name) {
    var G = window[name];
    if (!G) return;
    ['texImage2D', 'texSubImage2D'].forEach(function (method) {
      var f = G.prototype[method];
      if (!f) return;
      defineProperty(G.prototype, method, { configurable: true, writable: true, value: function () {
        var args = Array.prototype.slice.call(arguments);
        for (var i = 0; i < args.length; i++) if (args[i] && typeof args[i] === 'object' && managed.has(args[i])) args[i] = swap(args[i]);
        return f.apply(this, args);
      } });
    });
  });
  if (window.createImageBitmap) {
    var nativeBitmap = window.createImageBitmap;
    window.createImageBitmap = function (image) {
      var args = Array.prototype.slice.call(arguments);
      args[0] = swap(image);
      return nativeBitmap.apply(window, args);
    };
  }
  if (typeof VideoFrame !== 'undefined') {
    var NativeFrame = VideoFrame;
    var Frame = function (image, init) {
      if (image && managed.has(image)) {
        var st = stateOf(image);
        init = O.assign({ timestamp: Math.round(time(st) * 1e6) }, init || {});
        return new NativeFrame(used(st), init);
      }
      return arguments.length > 1 ? new NativeFrame(image, init) : new NativeFrame(image);
    };
    Frame.prototype = NativeFrame.prototype;
    O.setPrototypeOf(Frame, NativeFrame);
    window.VideoFrame = Frame;
  }
  if (VideoProto) {
    defineProperty(MediaProto, 'captureStream', { configurable: true, writable: true, value: function (rate) {
      var st = stateOf(this), c = used(st);
      st.capturing = true;
      return c.captureStream ? c.captureStream(rate) : null;
    } });
    // Called once for each picture of the film shown, with what it was.
    var frameCallbacks = new WeakMap(), nextHandle = 1;
    defineProperty(VideoProto, 'requestVideoFrameCallback', { configurable: true, writable: true, value: function (cb) {
      var st = stateOf(this), list = frameCallbacks.get(st);
      if (!list) { list = []; frameCallbacks.set(st, list); }
      var handle = nextHandle++;
      list.push({ handle: handle, cb: cb });
      watchFrames(st);
      return handle;
    } });
    defineProperty(VideoProto, 'cancelVideoFrameCallback', { configurable: true, writable: true, value: function (handle) {
      var list = frameCallbacks.get(stateOf(this));
      if (list) for (var i = 0; i < list.length; i++) if (list[i].handle === handle) list.splice(i, 1);
    } });
    var presented = new WeakMap();
    function watchFrames(st) {
      if (st.watching) return;
      st.watching = true;
      var last = -1;
      (function tick(stamp) {
        var list = frameCallbacks.get(st);
        if (!list || !list.length) { st.watching = false; return; }
        // Once for each of the film's pictures: its frame number, not the clock between them.
        var t = time(st), shown = Math.floor(t * (st.fps || 30) + 1e-6);
        if (shown !== last && st.readyState >= 2) {
          last = shown;
          var count = (presented.get(st) || 0) + 1;
          presented.set(st, count);
          var now_ = now(), calls = list.splice(0);
          var meta = { presentationTime: now_, expectedDisplayTime: now_ + 16, width: st.width, height: st.height, mediaTime: t, presentedFrames: count, processingDuration: 0 };
          calls.forEach(function (c) { try { c.cb.call(st.el, stamp, meta); } catch (e) { setTimeout_(function () { throw e; }); } });
        }
        raf(tick);
      })(now());
    }
  }

  // ---- step 4: MediaSource ----
  // A page that streams (YouTube, most players) appends pieces of a film to SourceBuffers of a
  // MediaSource. Both are SpatiMesh's: what is appended goes to the window, which reads it with
  // FFmpeg and plays it with the rest (spatiand_media::mse); appends finish, buffered ranges and
  // errors come back as the engine's would. A page that asks for a key system (DRM) keeps the
  // engine's own MediaSource from then on, and an element given keys goes back to the engine.
  var NativeMS = window.MediaSource || window.WebKitMediaSource;
  var nativeCreateURL = URL_.createObjectURL ? URL_.createObjectURL.bind(URL_) : null;
  var nativeRevokeURL = URL_.revokeObjectURL ? URL_.revokeObjectURL.bind(URL_) : null;
  var drm = false;
  if (navigator.requestMediaKeySystemAccess) {
    var nativeKeys = navigator.requestMediaKeySystemAccess.bind(navigator);
    navigator.requestMediaKeySystemAccess = function () { drm = true; return nativeKeys.apply(null, arguments); };
  }
  if (MediaProto.setMediaKeys) {
    var nativeSetKeys = MediaProto.setMediaKeys;
    defineProperty(MediaProto, 'setMediaKeys', { configurable: true, writable: true, value: function (keys) {
      if (keys && managed.has(this)) release(this, stateOf(this));
      return nativeSetKeys.call(this, keys);
    } });
  }
  var byMs = {};
  var nextMs = 1;
  var CODECS = /^(avc1|avc3|hev1|hvc1|dvh1|dvhe|vp8|vp9|vp09|vp08|av01|mp4a|opus|vorbis|flac|ac-3|ec-3|mp3|alac|theora)(\.|$)/i;
  function supported(type) {
    type = String(type || '');
    var base = type.split(';')[0].trim().toLowerCase();
    if (!/^(video|audio)\/(mp4|webm|x-matroska)$/.test(base)) return false;
    var codecs = /codecs\s*=\s*"?([^";]*)"?/i.exec(type);
    if (!codecs) return true;
    return codecs[1].split(',').every(function (c) { return CODECS.test(c.trim()); });
  }
  function onHandlers(proto, names) {
    names.forEach(function (n) {
      var key = '__on' + n;
      defineProperty(proto, 'on' + n, { configurable: true, enumerable: true,
        get: function () { return this[key] || null; },
        set: function (f) {
          if (this[key]) this.removeEventListener(n, this[key]);
          this[key] = typeof f === 'function' ? f : null;
          if (this[key]) this.addEventListener(n, this[key]);
        } });
    });
  }
  function emit(target, name) { var ev = new Event_(name); ours.add(ev); nativeDispatch.call(target, ev); }

  function List() { var self = new EventTarget(); O.setPrototypeOf(self, List.prototype); self.__items = []; return self; }
  List.prototype = O.create(EventTarget.prototype);
  defineProperty(List.prototype, 'length', { get: function () { return this.__items.length; } });
  onHandlers(List.prototype, ['addsourcebuffer', 'removesourcebuffer']);
  function listSet(list, items) {
    for (var i = 0; i < Math.max(items.length, list.__items.length) + 1; i++) delete list[i];
    list.__items = items.slice();
    items.forEach(function (sb, i) { defineProperty(list, i, { configurable: true, enumerable: true, get: function () { return list.__items[i]; } }); });
  }
  if (typeof SourceBufferList !== 'undefined') O.setPrototypeOf(List.prototype, SourceBufferList.prototype);

  function SB() { throw new TypeError('Illegal constructor'); }
  SB.prototype = O.create(EventTarget.prototype);
  onHandlers(SB.prototype, ['updatestart', 'update', 'updateend', 'error', 'abort']);
  var sbProps = {
    updating: function () { return this.__s.updating; },
    buffered: function () { return ranges(this.__s.buffered); },
    audioTracks: function () { return []; }, videoTracks: function () { return []; }, textTracks: function () { return []; }
  };
  O.keys(sbProps).forEach(function (k) { defineProperty(SB.prototype, k, { configurable: true, enumerable: true, get: sbProps[k] }); });
  defineProperty(SB.prototype, 'mode', { configurable: true, enumerable: true, get: function () { return this.__s.mode; }, set: function (v) {
    if (this.__s.updating) throw new DOMException('The SourceBuffer is updating.', 'InvalidStateError');
    this.__s.mode = v === 'sequence' ? 'sequence' : 'segments';
  } });
  defineProperty(SB.prototype, 'timestampOffset', { configurable: true, enumerable: true, get: function () { return this.__s.offset; }, set: function (v) {
    if (this.__s.updating) throw new DOMException('The SourceBuffer is updating.', 'InvalidStateError');
    this.__s.offset = Number(v) || 0;
    send({ t: 'ms', a: 'offset', ms: this.__s.ms.id, sb: this.__s.id, value: this.__s.offset });
  } });
  ['appendWindowStart', 'appendWindowEnd'].forEach(function (k, i) {
    defineProperty(SB.prototype, k, { configurable: true, enumerable: true, get: function () { return this.__s[k]; }, set: function (v) { this.__s[k] = Number(v); } });
  });
  function usable(sb) {
    var s = sb.__s;
    if (s.removed) throw new DOMException('This SourceBuffer has been removed from the parent media source.', 'InvalidStateError');
    if (s.updating) throw new DOMException('This SourceBuffer is still processing an appendBuffer or remove operation.', 'InvalidStateError');
    var ms = s.ms;
    if (ms.readyState === 'ended') { ms.readyState = 'open'; send({ t: 'ms', a: 'unend', ms: ms.id }); soon(function () { emit(ms.target, 'sourceopen'); }); }
  }
  defineProperty(SB.prototype, 'appendBuffer', { configurable: true, writable: true, value: function (data) {
    usable(this);
    var bytes = data instanceof ArrayBuffer ? new Uint8Array(data) : new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    var s = this.__s;
    s.updating = true;
    var self = this;
    soon(function () { emit(self, 'updatestart'); });
    send({ t: 'ms', a: 'append', ms: s.ms.id, sb: s.id, d: base64(bytes) });
  } });
  defineProperty(SB.prototype, 'remove', { configurable: true, writable: true, value: function (start, end) {
    usable(this);
    var s = this.__s;
    s.updating = true;
    var self = this;
    soon(function () { emit(self, 'updatestart'); });
    send({ t: 'ms', a: 'remove', ms: s.ms.id, sb: s.id, start: Number(start), end: Number(end) });
  } });
  defineProperty(SB.prototype, 'abort', { configurable: true, writable: true, value: function () {
    var s = this.__s;
    if (s.removed) throw new DOMException('This SourceBuffer has been removed from the parent media source.', 'InvalidStateError');
    send({ t: 'ms', a: 'abort', ms: s.ms.id, sb: s.id });
    if (s.updating) { s.updating = false; s.aborted = true; emit(this, 'abort'); emit(this, 'updateend'); }
    s.appendWindowStart = 0; s.appendWindowEnd = Infinity;
  } });
  defineProperty(SB.prototype, 'changeType', { configurable: true, writable: true, value: function (type) {
    usable(this);
    if (!supported(type)) throw new DOMException('The type provided (' + type + ') is unsupported.', 'NotSupportedError');
    send({ t: 'ms', a: 'type', ms: this.__s.ms.id, sb: this.__s.id, type: String(type) });
  } });
  if (typeof SourceBuffer !== 'undefined') O.setPrototypeOf(SB.prototype, SourceBuffer.prototype);

  function MS() {
    if (drm && NativeMS) return new NativeMS();
    var target = new EventTarget();
    O.setPrototypeOf(target, new.target ? new.target.prototype : MS.prototype);
    var self = { id: nextMs++, target: target, readyState: 'closed', duration: NaN, buffers: [], element: null, live: null };
    self.list = List(); self.active = List();
    target.__spatimesh = self;
    byMs[self.id] = self;
    self.attach = function (st) {
      self.element = st;
      self.readyState = 'open';
      soon(function () { emit(target, 'sourceopen'); if (target instanceof MMS) { target.__streaming = true; emit(target, 'startstreaming'); } });
    };
    self.detach = function () {
      self.element = null;
      self.readyState = 'closed';
      send({ t: 'ms', a: 'close', ms: self.id });
      soon(function () { emit(target, 'sourceclose'); });
    };
    send({ t: 'ms', a: 'new', ms: self.id });
    return target;
  }
  MS.prototype = O.create(EventTarget.prototype);
  onHandlers(MS.prototype, ['sourceopen', 'sourceended', 'sourceclose']);
  function inner(ms) { return ms.__spatimesh; }
  defineProperty(MS.prototype, 'readyState', { configurable: true, enumerable: true, get: function () { return inner(this).readyState; } });
  defineProperty(MS.prototype, 'sourceBuffers', { configurable: true, enumerable: true, get: function () { return inner(this).list; } });
  defineProperty(MS.prototype, 'activeSourceBuffers', { configurable: true, enumerable: true, get: function () { return inner(this).active; } });
  defineProperty(MS.prototype, 'duration', { configurable: true, enumerable: true, get: function () { return inner(this).duration; }, set: function (v) {
    var self = inner(this);
    v = Number(v);
    if (!(v >= 0)) throw new TypeError("The provided double value is negative or not a number.");
    if (self.readyState !== 'open') throw new DOMException("The MediaSource's readyState is not 'open'.", 'InvalidStateError');
    if (self.buffers.some(function (b) { return b.__s.updating; })) throw new DOMException('A SourceBuffer is updating.', 'InvalidStateError');
    self.duration = v;
    send({ t: 'ms', a: 'duration', ms: self.id, value: v === Infinity ? -1 : v });
  } });
  defineProperty(MS.prototype, 'addSourceBuffer', { configurable: true, writable: true, value: function (type) {
    var self = inner(this);
    if (!supported(type)) throw new DOMException('The type provided (' + type + ') is unsupported.', 'NotSupportedError');
    if (self.readyState !== 'open') throw new DOMException("The MediaSource's readyState is not 'open'.", 'InvalidStateError');
    var sb = new EventTarget();
    O.setPrototypeOf(sb, SB.prototype);
    sb.__s = { id: self.buffers.length + 1, ms: self, updating: false, buffered: [], mode: 'segments', offset: 0, appendWindowStart: 0, appendWindowEnd: Infinity };
    self.buffers.push(sb);
    listSet(self.list, self.buffers); listSet(self.active, self.buffers);
    send({ t: 'ms', a: 'add', ms: self.id, sb: sb.__s.id, type: String(type) });
    soon(function () { emit(self.list, 'addsourcebuffer'); emit(self.active, 'addsourcebuffer'); });
    return sb;
  } });
  defineProperty(MS.prototype, 'removeSourceBuffer', { configurable: true, writable: true, value: function (sb) {
    var self = inner(this), i = self.buffers.indexOf(sb);
    if (i < 0) throw new DOMException('The SourceBuffer provided is not contained in this MediaSource.', 'NotFoundError');
    if (sb.__s.updating) { sb.__s.updating = false; emit(sb, 'abort'); emit(sb, 'updateend'); }
    sb.__s.removed = true;
    self.buffers.splice(i, 1);
    listSet(self.list, self.buffers); listSet(self.active, self.buffers);
    send({ t: 'ms', a: 'removesb', ms: self.id, sb: sb.__s.id });
    soon(function () { emit(self.list, 'removesourcebuffer'); emit(self.active, 'removesourcebuffer'); });
  } });
  defineProperty(MS.prototype, 'endOfStream', { configurable: true, writable: true, value: function (error) {
    var self = inner(this);
    if (self.readyState !== 'open') throw new DOMException("The MediaSource's readyState is not 'open'.", 'InvalidStateError');
    if (self.buffers.some(function (b) { return b.__s.updating; })) throw new DOMException('A SourceBuffer is updating.', 'InvalidStateError');
    self.readyState = 'ended';
    send({ t: 'ms', a: 'end', ms: self.id, error: error || '' });
    soon(function () { emit(self.target, 'sourceended'); });
    if (error && self.element) {
      var st = self.element;
      st.error = mediaError(error === 'network' ? 2 : 3, 'MediaSource ended with a ' + error + ' error');
      fire(st.el, 'error');
    }
  } });
  defineProperty(MS.prototype, 'setLiveSeekableRange', { configurable: true, writable: true, value: function (a, b) { inner(this).live = [Number(a), Number(b)]; } });
  defineProperty(MS.prototype, 'clearLiveSeekableRange', { configurable: true, writable: true, value: function () { inner(this).live = null; } });
  MS.isTypeSupported = function (type) { return drm && NativeMS ? NativeMS.isTypeSupported(type) : supported(type); };
  MS.canConstructInDedicatedWorker = false;
  if (NativeMS) O.setPrototypeOf(MS.prototype, NativeMS.prototype);
  // Safari's: the same, which says it is streaming once it is attached.
  function MMS() { return Reflect.construct(MS, [], MMS); }
  MMS.prototype = O.create(MS.prototype);
  onHandlers(MMS.prototype, ['startstreaming', 'endstreaming']);
  defineProperty(MMS.prototype, 'streaming', { configurable: true, enumerable: true, get: function () { return !!this.__streaming; } });
  MMS.isTypeSupported = MS.isTypeSupported;
  window.MediaSource = MS;
  if (window.WebKitMediaSource) window.WebKitMediaSource = MS;
  if (window.ManagedMediaSource) window.ManagedMediaSource = MMS;
  if (nativeCreateURL) {
    URL_.createObjectURL = function (obj) {
      if (obj && obj.__spatimesh) {
        var url = nativeCreateURL(new Blob([]));
        sources[url] = obj;
        return url;
      }
      return nativeCreateURL(obj);
    };
    URL_.revokeObjectURL = function (url) { delete sources[url]; return nativeRevokeURL(url); };
  }

  function msReceive(m) {
    var self = byMs[m.ms];
    if (!self) return;
    var sb = null;
    for (var i = 0; i < self.buffers.length; i++) if (self.buffers[i].__s.id === m.sb) sb = self.buffers[i];
    if (m.a === 'duration') { if (m.value > 0) self.duration = m.value; return; }
    if (!sb) return;
    var s = sb.__s;
    if (m.buffered) s.buffered = m.buffered;
    if (m.a === 'appended' || m.a === 'removed') {
      if (s.aborted) { s.aborted = false; return; }
      if (!s.updating) return;
      s.updating = false;
      if (m.error) {
        emit(sb, 'error');
        emit(sb, 'updateend');
        if (m.error === 'QuotaExceededError') return;
        if (self.readyState === 'open') try { self.target.endOfStream('decode'); } catch (e) {}
        return;
      }
      emit(sb, 'update');
      emit(sb, 'updateend');
    }
  }

  // ---- step 2: Web Audio, to the room ----
  // Every AudioContext is made as it always is, but what the page connects to its destination
  // reaches a node of ours instead, which hands the mix to the window (the room, at this window)
  // and gives the engine's own output silence. OfflineAudioContext renders to the page, not to a
  // speaker, and is left alone.
  function audioReceive(m) {}
  var nextAudio = 1;
  var b64chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  function base64(bytes) {
    var out = '', i = 0, n = bytes.length;
    for (; i + 2 < n; i += 3) {
      var v = (bytes[i] << 16) | (bytes[i + 1] << 8) | bytes[i + 2];
      out += b64chars[v >> 18] + b64chars[(v >> 12) & 63] + b64chars[(v >> 6) & 63] + b64chars[v & 63];
    }
    if (i < n) {
      var w = bytes[i] << 16 | (i + 1 < n ? bytes[i + 1] << 8 : 0);
      out += b64chars[w >> 18] + b64chars[(w >> 12) & 63] + (i + 1 < n ? b64chars[(w >> 6) & 63] : '=') + '=';
    }
    return out;
  }
  function wrapAudio(Native) {
    if (!Native || !Native.prototype) return Native;
    var destination = native(Native.prototype, 'destination');
    function Wrapped(options) {
      var ctx = arguments.length ? new Native(options) : new Native();
      try { capture(ctx, destination); } catch (e) {}
      return ctx;
    }
    Wrapped.prototype = Native.prototype;
    O.setPrototypeOf(Wrapped, Native);
    try { defineProperty(Wrapped, 'name', { value: Native.name }); } catch (e) {}
    try { defineProperty(Native.prototype, 'constructor', { value: Wrapped, configurable: true, writable: true }); } catch (e) {}
    return Wrapped;
  }
  function capture(ctx, destination) {
    var real = destination.get.call(ctx);
    if (!ctx.createScriptProcessor) return;
    var id = nextAudio++, opened = false, frames = 2048;
    var sink = ctx.createGain();
    var tap = ctx.createScriptProcessor(frames, 2, 2);
    var quiet = ctx.createGain();
    quiet.gain.value = 0;
    sink.connect(tap); tap.connect(quiet); quiet.connect(real);
    ['maxChannelCount'].forEach(function (k) { try { defineProperty(sink, k, { get: function () { return real[k]; } }); } catch (e) {} });
    defineProperty(ctx, 'destination', { configurable: true, get: function () { return sink; } });
    var pcm = new Int16Array(frames * 2), bytes = new Uint8Array(pcm.buffer);
    tap.onaudioprocess = function (ev) {
      var input = ev.inputBuffer, l = input.getChannelData(0), r = input.numberOfChannels > 1 ? input.getChannelData(1) : l;
      var loud = false, n = l.length;
      if (pcm.length !== n * 2) { pcm = new Int16Array(n * 2); bytes = new Uint8Array(pcm.buffer); }
      for (var i = 0; i < n; i++) {
        var a = l[i], b = r[i];
        if (a !== 0 || b !== 0) loud = true;
        pcm[i * 2] = a <= -1 ? -32768 : a >= 1 ? 32767 : a * 32767;
        pcm[i * 2 + 1] = b <= -1 ? -32768 : b >= 1 ? 32767 : b * 32767;
      }
      if (!loud) return;
      if (!opened) { opened = true; send({ t: 'audio', a: 'open', id: id, rate: ctx.sampleRate, channels: 2 }); }
      send({ t: 'audio', a: 'data', id: id, rate: ctx.sampleRate, d: base64(bytes) });
    };
    nativeAddListener.call(ctx, 'statechange', function () {
      if (ctx.state === 'closed' && opened) { opened = false; send({ t: 'audio', a: 'close', id: id }); }
    });
  }
  if (window.AudioContext) window.AudioContext = wrapAudio(window.AudioContext);
  if (window.webkitAudioContext) window.webkitAudioContext = wrapAudio(window.webkitAudioContext);

  // ---- fullscreen ----
  // The engines' own fullscreen is no use here (WebKit's is not even offered to pages in an
  // embedded view), so it is SpatiMesh's: the element is fixed over the whole of its frame's
  // viewport, a frame's element over its parent's, and the window makes the page the whole window.
  // The page sees requestFullscreen, fullscreenElement, fullscreenEnabled, exitFullscreen and the
  // fullscreenchange events as a browser's (WebKit's prefixed names too).
  var fsElement = null, fsSaved = null, fsChild = null;
  var FS = { position: 'fixed', top: '0', left: '0', right: '0', bottom: '0', width: '100vw', height: '100vh', 'max-width': 'none',
    'max-height': 'none', 'min-width': '0', 'min-height': '0', margin: '0', 'z-index': '2147483647', transform: 'none', 'box-sizing': 'border-box' };
  function fsEvents(el) {
    ['fullscreenchange', 'webkitfullscreenchange'].forEach(function (name) {
      var ev = new Event_(name, { bubbles: true, composed: true });
      ours.add(ev);
      nativeDispatch.call(el && el.isConnected ? el : document, ev);
    });
  }
  function enterFs(el, child) {
    if (fsElement === el) return Promise_.resolve();
    if (fsElement) leaveFs(true);
    fsSaved = O.keys(FS).concat(['background-color']).map(function (k) { return [k, el.style.getPropertyValue(k), el.style.getPropertyPriority(k)]; });
    O.keys(FS).forEach(function (k) { el.style.setProperty(k, FS[k], 'important'); });
    // What is behind it is not to be seen, as a browser's backdrop is black.
    var bg = nativeGetComputed(el).backgroundColor;
    if (!managed.has(el) && (bg === 'rgba(0, 0, 0, 0)' || bg === 'transparent')) el.style.setProperty('background-color', '#000', 'important');
    fsElement = el;
    fsChild = child || null;
    send({ t: 'fullscreen', on: true });
    soon(function () { fsEvents(el); places(); });
    return Promise_.resolve();
  }
  function leaveFs(quiet, fromBelow) {
    var el = fsElement;
    if (!el) return Promise_.resolve();
    (fsSaved || []).forEach(function (s) { if (s[1]) el.style.setProperty(s[0], s[1], s[2]); else el.style.removeProperty(s[0]); });
    fsElement = null; fsSaved = null;
    if (fsChild && !fromBelow) postMessage_(fsChild, { __spatimeshMediaDown: { t: 'fullscreen-exit' } });
    fsChild = null;
    if (!quiet) send({ t: 'fullscreen', on: false });
    soon(function () { fsEvents(el); places(); });
    return Promise_.resolve();
  }
  function fsMethod(proto, name, f) { try { defineProperty(proto, name, { configurable: true, writable: true, value: f }); } catch (e) {} }
  function fsGetter(proto, name, f) { try { defineProperty(proto, name, { configurable: true, enumerable: true, get: f }); } catch (e) {} }
  ['requestFullscreen', 'webkitRequestFullscreen', 'webkitRequestFullScreen'].forEach(function (n) {
    fsMethod(ElementProto, n, function () { return enterFs(this); });
  });
  if (VideoProto) ['webkitEnterFullscreen', 'webkitEnterFullScreen'].forEach(function (n) { fsMethod(VideoProto, n, function () { enterFs(this); }); });
  ['exitFullscreen', 'webkitExitFullscreen', 'webkitCancelFullScreen'].forEach(function (n) {
    fsMethod(Document.prototype, n, function () { return leaveFs(); });
  });
  ['fullscreenElement', 'webkitFullscreenElement', 'webkitCurrentFullScreenElement'].forEach(function (n) {
    fsGetter(Document.prototype, n, function () { return fsElement; });
  });
  ['fullscreenEnabled', 'webkitFullscreenEnabled', 'webkitFullScreenKeyboardInputAllowed'].forEach(function (n) {
    fsGetter(Document.prototype, n, function () { return true; });
  });
  ['fullscreen', 'webkitIsFullScreen'].forEach(function (n) { fsGetter(Document.prototype, n, function () { return !!fsElement; }); });
  if (VideoProto) fsGetter(VideoProto, 'webkitDisplayingFullscreen', function () { return fsElement === this; });
  // The handler properties too: players look for `onfullscreenchange in document` to know the
  // API is there at all.
  var fsHandlers = new WeakMap();
  ['fullscreenchange', 'fullscreenerror', 'webkitfullscreenchange', 'webkitfullscreenerror'].forEach(function (type) {
    [Document.prototype, ElementProto].forEach(function (proto) {
      if (('on' + type) in proto) return;
      try {
        defineProperty(proto, 'on' + type, { configurable: true, enumerable: true,
          get: function () { var h = fsHandlers.get(this); return (h && h[type] && h[type].f) || null; },
          set: function (f) {
            var h = fsHandlers.get(this);
            if (!h) fsHandlers.set(this, h = {});
            if (h[type]) nativeRemoveListener.call(this, type, h[type].call);
            h[type] = null;
            if (typeof f !== 'function') return;
            var target = this;
            h[type] = { f: f, call: function (ev) { return f.call(target, ev); } };
            nativeAddListener.call(this, type, h[type].call);
          } });
      } catch (e) {}
    });
  });
  // Fullscreen in a browser makes the viewport the screen, and players check for that.
  if (window.Screen) ['width', 'height', 'availWidth', 'availHeight'].forEach(function (n) {
    var d = O.getOwnPropertyDescriptor(Screen.prototype, n);
    if (!d || !d.get) return;
    fsGetter(Screen.prototype, n, function () {
      if (!fsElement) return d.get.call(this);
      return n === 'width' || n === 'availWidth' ? innerWidth : innerHeight;
    });
  });

  // ---- this document, to the window: here, and gone ----
  send({ t: 'hello', top: TOP });
  nativeAddListener.call(window, 'pagehide', function () { send({ t: 'bye' }); }, true);

  // STEPS
})();
