// SpatiMesh's WebExtensions API: `chrome`, and `browser` for Firefox's extensions, in every
// extension context -- content scripts, background pages, popups, options and other pages.
// Every call is answered by SpatiMesh's window (crates/spatimesh/src/extensions/runtime.rs):
// it goes out as JSON over the engine's bridge (`post`), and replies and events come back to
// `__spatimeshExt.deliver`. Modelled on Chromium's and Firefox's own (their extension bindings).
//
// `config`: {id, runtimeId, token, kind ("content" | "background" | "popup" | "options" |
// "page"), flavor ("chrome" | "firefox"), manifest, messages, uiLanguage, origin, api: {name:
// true}, version, incognito}.
function __spatimeshExtBoot(config, post) {
  'use strict';
  var JSONstringify = JSON.stringify;
  var JSONparse = JSON.parse;
  var PromiseC = Promise;
  var slice = Array.prototype.slice;
  var isContent = config.kind === 'content';
  var frameId = window.top === window ? 0 : Math.floor(Math.random() * 2147483646) + 1;
  var contextId = Math.random().toString(36).slice(2) + Date.now().toString(36);
  var nextCall = 1;
  var nextPort = 1;
  var pending = {};
  var events = {};
  var ports = {};
  var lastError = null;
  var lastErrorChecked = false;

  function say(call, args, id) {
    post(JSONstringify({ t: config.token, e: config.id, k: config.kind, f: frameId, x: contextId,
      u: String(location.href), c: call, a: args || [], i: id }));
  }

  function call(name, args) {
    return new PromiseC(function (resolve, reject) {
      var id = nextCall++;
      pending[id] = { resolve: resolve, reject: reject };
      say(name, args, id);
    });
  }

  // runtime.lastError: set while a callback runs for a call that failed, and complained
  // about if the callback never looked, as Chrome does.
  function withLastError(error, fn) {
    lastError = { message: String(error && error.message || error) };
    lastErrorChecked = false;
    try { fn(); } finally {
      if (!lastErrorChecked) console.error('Unchecked runtime.lastError: ' + lastError.message);
      lastError = null;
    }
  }

  // A method answered by the window: a promise, or the callback given last.
  function method(name, prepare, after) {
    return function () {
      var args = slice.call(arguments);
      var callback = typeof args[args.length - 1] === 'function' ? args.pop() : null;
      if (prepare) args = prepare(args);
      var p = call(name, args);
      if (after) p = p.then(after);
      if (callback) {
        p.then(function (v) { callback(v); }, function (e) { withLastError(e, function () { callback(); }); });
        return undefined;
      }
      return p;
    };
  }

  // A method answered here, in the same two ways.
  function local(fn) {
    return function () {
      var args = slice.call(arguments);
      var callback = typeof args[args.length - 1] === 'function' ? args.pop() : null;
      var p;
      try { p = PromiseC.resolve(fn.apply(null, args)); } catch (e) { p = PromiseC.reject(e); }
      if (callback) {
        p.then(function (v) { callback(v); }, function (e) { withLastError(e, function () { callback(); }); });
        return undefined;
      }
      return p;
    };
  }

  function makeEvent(name, quiet) {
    var list = [];
    var e = {
      addListener: function (fn) {
        if (typeof fn !== 'function' || list.indexOf(fn) >= 0) return;
        list.push(fn);
        if (!quiet) say('listen', [name, true].concat(slice.call(arguments, 1)));
      },
      removeListener: function (fn) {
        var i = list.indexOf(fn);
        if (i < 0) return;
        list.splice(i, 1);
        if (!quiet && !list.length) say('listen', [name, false]);
      },
      hasListener: function (fn) { return list.indexOf(fn) >= 0; },
      hasListeners: function () { return list.length > 0; },
      _fire: function (args) {
        var results = [];
        list.slice().forEach(function (fn) {
          try { results.push(fn.apply(null, args)); } catch (err) { console.error(err); }
        });
        return results;
      },
      _listeners: list
    };
    if (!quiet) events[name] = e;
    return e;
  }

  function namespace(name, methods, eventNames, extra) {
    var ns = {};
    (methods || []).forEach(function (m) { ns[m] = method(name + '.' + m); });
    (eventNames || []).forEach(function (ev) { ns[ev] = makeEvent(name + '.' + ev); });
    if (extra) Object.keys(extra).forEach(function (k) { ns[k] = extra[k]; });
    return ns;
  }

  // ---- messages and ports --------------------------------------------------------------

  function makePort(id, name, sender) {
    var port = {
      name: name || '',
      sender: sender,
      onMessage: makeEvent('port.onMessage', true),
      onDisconnect: makeEvent('port.onDisconnect', true),
      postMessage: function (message) {
        if (port._gone) throw new Error('Attempting to use a disconnected port object');
        say('port.post', [id, message === undefined ? null : message]);
      },
      disconnect: function () {
        if (port._gone) return;
        port._gone = true;
        delete ports[id];
        say('port.disconnect', [id]);
      }
    };
    if (!sender) delete port.sender;
    ports[id] = port;
    return port;
  }

  function connect(kind, target, info) {
    info = info || {};
    var id = contextId + '.' + nextPort++;
    var port = makePort(id, info.name);
    say(kind, [id, info.name || '', target, info]);
    return port;
  }

  // A message for onMessage's listeners: the first sendResponse answers, `return true` or a
  // promise keeps the channel open for a later one.
  function receiveMessage(eventName, args, q) {
    var e = events[eventName];
    var fns = e ? e._listeners.slice() : [];
    if (!fns.length) { say('message.none', [q, false]); return; }
    var answered = false;
    var open = false;
    function sendResponse(response) {
      if (answered) return;
      answered = true;
      say('message.respond', [q, response === undefined ? null : response]);
    }
    fns.forEach(function (fn) {
      var r;
      try { r = fn(args[0], args[1], sendResponse); } catch (err) { console.error(err); }
      if (r === true) open = true;
      else if (r && typeof r.then === 'function') {
        open = true;
        r.then(function (v) { sendResponse(v); }, function (err) {
          if (!answered) { answered = true; say('message.error', [q, String(err && err.message || err)]); }
        });
      }
    });
    if (!answered && !open) say('message.none', [q, true]);
  }

  function dispatch(name, args, q) {
    switch (name) {
      case 'runtime.onMessage':
      case 'runtime.onMessageExternal':
        receiveMessage(name, args, q);
        return;
      case 'runtime.onConnect':
      case 'runtime.onConnectExternal': {
        var port = makePort(args[0], args[1], args[2]);
        var e = events[name];
        if (e) e._fire([port]);
        return;
      }
      case 'port.message': {
        var p = ports[args[0]];
        if (p) p.onMessage._fire([args[1], p]);
        return;
      }
      case 'port.disconnect': {
        var gone = ports[args[0]];
        if (!gone) return;
        gone._gone = true;
        delete ports[args[0]];
        if (args[1]) withLastError(args[1], function () { gone.onDisconnect._fire([gone]); });
        else gone.onDisconnect._fire([gone]);
        return;
      }
      case 'storage.onChanged': {
        if (events['storage.onChanged']) events['storage.onChanged']._fire([args[0], args[1]]);
        var areaEvent = events['storage.' + args[1] + '.onChanged'];
        if (areaEvent) areaEvent._fire([args[0]]);
        return;
      }
    }
    var ev = events[name];
    if (!ev) { if (q !== undefined) say('event.result', [q, null]); return; }
    var results = ev._fire(args);
    // An event whose listeners answer it (webRequest's blocking ones): the first answer.
    if (q !== undefined) {
      var answer = null;
      for (var i = 0; i < results.length; i++) if (results[i] !== undefined) { answer = results[i]; break; }
      PromiseC.resolve(answer).then(function (v) { say('event.result', [q, v === undefined ? null : v]); },
        function () { say('event.result', [q, null]); });
    }
  }

  function deliver(json) {
    var m = typeof json === 'string' ? JSONparse(json) : json;
    if (m.f !== undefined && m.f !== null && m.f !== frameId) return;
    if (m.r !== undefined) {
      var p = pending[m.r];
      if (!p) return;
      delete pending[m.r];
      if (m.x !== undefined && m.x !== null) p.reject(new Error(m.x)); else p.resolve(m.v);
      return;
    }
    if (m.ev) dispatch(m.ev, m.a || [], m.q);
  }

  // ---- the namespaces ----------------------------------------------------------------------

  var manifest = config.manifest || {};
  var has = config.api || {};
  var origin = config.origin;
  function getURL(path) {
    path = String(path || '');
    if (/^[a-z-]+:\/\//.test(path)) return path;
    return origin + '/' + path.replace(/^\/+/, '');
  }
  function getMessage(name, substitutions) {
    if (substitutions !== undefined && substitutions !== null && !Array.isArray(substitutions)) substitutions = [substitutions];
    var subs = (substitutions || []).map(String);
    var key = String(name).toLowerCase();
    var fixed = { '@@extension_id': config.id, '@@ui_locale': String(config.uiLanguage).replace('-', '_'),
      '@@bidi_dir': 'ltr', '@@bidi_reversed_dir': 'rtl', '@@bidi_start_edge': 'left', '@@bidi_end_edge': 'right' };
    if (fixed[key] !== undefined) return fixed[key];
    var m = (config.messages || {})[key];
    if (!m) return '';
    var placeholders = m.placeholders || {};
    var text = String(m.message).replace(/\$([A-Za-z0-9_@]+)\$/g, function (whole, ph) {
      var c = placeholders[ph.toLowerCase()];
      return c === undefined ? whole : c;
    });
    return text.replace(/\$(\$|[1-9])/g, function (whole, d) { return d === '$' ? '$' : (subs[+d - 1] !== undefined ? subs[+d - 1] : ''); });
  }

  var runtime = {
    id: config.runtimeId || config.id,
    getURL: getURL,
    getManifest: function () { return JSONparse(JSONstringify(manifest)); },
    sendMessage: function () {
      var args = slice.call(arguments);
      var callback = typeof args[args.length - 1] === 'function' ? args.pop() : null;
      // (extensionId?, message, options?): an id first only if it is a string and more follows.
      var target = null;
      if (args.length >= 2 && (typeof args[0] === 'string' || args[0] === null) && !(args.length === 2 && typeof args[1] === 'object' && args[1] !== null && !Array.isArray(args[1]) && 'includeTlsChannelId' in args[1])) target = args.shift();
      var message = args[0];
      var p = call('runtime.sendMessage', [target, message === undefined ? null : message]);
      if (callback) {
        p.then(function (v) { callback(v); }, function (e) { withLastError(e, function () { callback(); }); });
        return undefined;
      }
      return p;
    },
    connect: function (a, b) {
      if (typeof a === 'string') return connect('runtime.connect', a, b);
      return connect('runtime.connect', null, a);
    },
    onMessage: makeEvent('runtime.onMessage'),
    onConnect: makeEvent('runtime.onConnect'),
    onMessageExternal: makeEvent('runtime.onMessageExternal'),
    onConnectExternal: makeEvent('runtime.onConnectExternal')
  };
  Object.defineProperty(runtime, 'lastError', { enumerable: true, get: function () { lastErrorChecked = true; return lastError || undefined; } });

  var i18n = {
    getMessage: getMessage,
    getUILanguage: function () { return config.uiLanguage; },
    getAcceptLanguages: local(function () { return [config.uiLanguage, String(config.uiLanguage).split('-')[0]]; }),
    detectLanguage: local(function () { return { isReliable: false, languages: [] }; })
  };

  function storageArea(area) {
    return namespace('storage.' + area, ['get', 'set', 'remove', 'clear', 'getBytesInUse', 'getKeys', 'setAccessLevel'], ['onChanged'],
      area === 'sync' ? { QUOTA_BYTES: 102400, QUOTA_BYTES_PER_ITEM: 8192, MAX_ITEMS: 512, MAX_WRITE_OPERATIONS_PER_HOUR: 1800, MAX_WRITE_OPERATIONS_PER_MINUTE: 120 }
        : area === 'local' ? { QUOTA_BYTES: 10485760 } : area === 'session' ? { QUOTA_BYTES: 10485760 } : {});
  }
  function storageNs() {
    return { local: storageArea('local'), sync: storageArea('sync'), session: storageArea('session'), managed: storageArea('managed'),
      onChanged: makeEvent('storage.onChanged') };
  }

  var extension = {
    getURL: getURL,
    inIncognitoContext: !!config.incognito,
    isAllowedIncognitoAccess: local(function () { return false; }),
    isAllowedFileSchemeAccess: local(function () { return false; })
  };

  var chromeApi = { runtime: runtime, i18n: i18n, extension: extension };
  if (has.storage) chromeApi.storage = storageNs();

  if (!isContent) {
    runtime.getPlatformInfo = local(function () { return { os: config.os || 'linux', arch: 'x86-64', nacl_arch: 'x86-64' }; });
    runtime.getBrowserInfo = local(function () { return { name: 'SpatiMesh', vendor: 'Spatiand', version: config.version || '1.0', buildID: '0' }; });
    runtime.openOptionsPage = method('runtime.openOptionsPage');
    runtime.setUninstallURL = local(function () {});
    runtime.reload = function () { say('runtime.reload', []); };
    runtime.requestUpdateCheck = local(function () { return { status: 'no_update' }; });
    runtime.getContexts = method('runtime.getContexts');
    runtime.onInstalled = makeEvent('runtime.onInstalled');
    runtime.onStartup = makeEvent('runtime.onStartup');
    runtime.onSuspend = makeEvent('runtime.onSuspend');
    runtime.onSuspendCanceled = makeEvent('runtime.onSuspendCanceled');
    runtime.onUpdateAvailable = makeEvent('runtime.onUpdateAvailable');
    runtime.onRestartRequired = makeEvent('runtime.onRestartRequired');
    runtime.getBackgroundPage = local(function () { return null; });
    runtime.OnInstalledReason = { INSTALL: 'install', UPDATE: 'update', CHROME_UPDATE: 'chrome_update', SHARED_MODULE_UPDATE: 'shared_module_update' };
    runtime.PlatformOs = { MAC: 'mac', WIN: 'win', ANDROID: 'android', CROS: 'cros', LINUX: 'linux', OPENBSD: 'openbsd' };
    extension.getViews = function () { return []; };
    extension.getBackgroundPage = function () { return null; };

    var tabMethods = ['query', 'get', 'getCurrent', 'create', 'update', 'remove', 'reload', 'goBack', 'goForward', 'duplicate',
      'highlight', 'move', 'discard', 'captureVisibleTab', 'detectLanguage', 'getZoom', 'setZoom', 'getZoomSettings',
      'setZoomSettings', 'executeScript', 'insertCSS', 'removeCSS', 'group', 'ungroup'];
    chromeApi.tabs = namespace('tabs', tabMethods,
      ['onCreated', 'onUpdated', 'onActivated', 'onRemoved', 'onMoved', 'onHighlighted', 'onAttached', 'onDetached', 'onReplaced', 'onZoomChange'],
      { TAB_ID_NONE: -1, TAB_INDEX_NONE: -1, MAX_CAPTURE_VISIBLE_TAB_CALLS_PER_SECOND: 2 });
    chromeApi.tabs.sendMessage = function () {
      var args = slice.call(arguments);
      var callback = typeof args[args.length - 1] === 'function' ? args.pop() : null;
      var p = call('tabs.sendMessage', [args[0], args[1] === undefined ? null : args[1], args[2] || {}]);
      if (callback) {
        p.then(function (v) { callback(v); }, function (e) { withLastError(e, function () { callback(); }); });
        return undefined;
      }
      return p;
    };
    chromeApi.tabs.connect = function (tabId, info) { return connect('tabs.connect', tabId, info); };
    chromeApi.windows = namespace('windows', ['get', 'getAll', 'getCurrent', 'getLastFocused', 'create', 'update', 'remove'],
      ['onCreated', 'onRemoved', 'onFocusChanged', 'onBoundsChanged'], { WINDOW_ID_NONE: -1, WINDOW_ID_CURRENT: -2 });

    if (has.scripting) {
      chromeApi.scripting = namespace('scripting', ['insertCSS', 'removeCSS', 'registerContentScripts', 'getRegisteredContentScripts',
        'unregisterContentScripts', 'updateContentScripts'], [], { ExecutionWorld: { ISOLATED: 'ISOLATED', MAIN: 'MAIN' } });
      chromeApi.scripting.executeScript = method('scripting.executeScript', function (args) {
        var d = Object.assign({}, args[0] || {});
        if (typeof d.func === 'function') d.func = d.func.toString();
        if (typeof d.function === 'function') { d.func = d.function.toString(); delete d.function; }
        return [d];
      });
    }
    var actionMethods = ['setTitle', 'getTitle', 'setIcon', 'setPopup', 'getPopup', 'setBadgeText', 'getBadgeText',
      'setBadgeBackgroundColor', 'getBadgeBackgroundColor', 'setBadgeTextColor', 'getBadgeTextColor', 'enable', 'disable',
      'isEnabled', 'openPopup', 'getUserSettings'];
    function action(name) {
      var ns = namespace(name, actionMethods, ['onClicked', 'onUserSettingsChanged']);
      ns.setIcon = method(name + '.setIcon', function (args) {
        var d = Object.assign({}, args[0] || {});
        // Pictures made in a canvas: their pixels, so that they can cross.
        function plain(img) { return img && img.data ? { width: img.width, height: img.height, data: Array.prototype.slice.call(img.data) } : img; }
        if (d.imageData) {
          if (d.imageData.data) d.imageData = plain(d.imageData);
          else Object.keys(d.imageData).forEach(function (k) { d.imageData[k] = plain(d.imageData[k]); });
        }
        return [d];
      });
      return ns;
    }
    if (manifest.action) chromeApi.action = action('action');
    if (manifest.browser_action) chromeApi.browserAction = action('browserAction');
    if (manifest.page_action) {
      chromeApi.pageAction = action('pageAction');
      chromeApi.pageAction.show = method('pageAction.show');
      chromeApi.pageAction.hide = method('pageAction.hide');
    }
    if (has.alarms) chromeApi.alarms = namespace('alarms', ['get', 'getAll', 'clear', 'clearAll'], ['onAlarm'], {
      create: function (name, info) {
        if (typeof name !== 'string') { info = name; name = ''; }
        return call('alarms.create', [name, info || {}]);
      }
    });
    if (has.notifications) chromeApi.notifications = namespace('notifications', ['update', 'clear', 'getAll', 'getPermissionLevel'],
      ['onClicked', 'onClosed', 'onButtonClicked', 'onPermissionLevelChanged', 'onShowSettings'], {
        create: method('notifications.create', function (args) {
          if (typeof args[0] !== 'string') args.unshift('');
          return [args[0], args[1] || {}];
        })
      });
    chromeApi.permissions = namespace('permissions', ['contains', 'getAll', 'request', 'remove', 'addHostAccessRequest', 'removeHostAccessRequest'],
      ['onAdded', 'onRemoved']);
    if (has.contextMenus || has.menus) {
      var menus = namespace('contextMenus', ['update', 'remove', 'removeAll', 'refresh'], ['onClicked', 'onShown', 'onHidden'], {
        ACTION_MENU_TOP_LEVEL_LIMIT: 6,
        create: function (props, callback) {
          props = Object.assign({}, props || {});
          var id = props.id !== undefined ? props.id : (contextId + '-' + nextCall++);
          props.id = id;
          if (typeof props.onclick === 'function') {
            var handler = props.onclick;
            delete props.onclick;
            menus.onClicked.addListener(function (info, tab) { if (info.menuItemId === id) handler(info, tab); });
          }
          call('contextMenus.create', [props]).then(function () { if (callback) callback(); },
            function (e) { if (callback) withLastError(e, function () { callback(); }); });
          return id;
        }
      });
      if (has.contextMenus) chromeApi.contextMenus = menus;
      if (has.menus) chromeApi.menus = menus;
    }
    if (has.webNavigation) chromeApi.webNavigation = namespace('webNavigation', ['getFrame', 'getAllFrames'],
      ['onBeforeNavigate', 'onCommitted', 'onDOMContentLoaded', 'onCompleted', 'onErrorOccurred', 'onCreatedNavigationTarget',
        'onReferenceFragmentUpdated', 'onTabReplaced', 'onHistoryStateUpdated']);
    if (has.webRequest) chromeApi.webRequest = namespace('webRequest', ['handlerBehaviorChanged'],
      ['onBeforeRequest', 'onBeforeSendHeaders', 'onSendHeaders', 'onHeadersReceived', 'onAuthRequired', 'onResponseStarted',
        'onBeforeRedirect', 'onCompleted', 'onErrorOccurred'], { MAX_HANDLER_BEHAVIOR_CHANGED_CALLS_PER_10_MINUTES: 20 });
    if (has.declarativeNetRequest || has.declarativeNetRequestWithHostAccess) chromeApi.declarativeNetRequest = namespace('declarativeNetRequest',
      ['updateDynamicRules', 'getDynamicRules', 'updateSessionRules', 'getSessionRules', 'updateEnabledRulesets', 'getEnabledRulesets',
        'updateStaticRules', 'getDisabledRuleIds', 'getAvailableStaticRuleCount', 'isRegexSupported', 'setExtensionActionOptions',
        'getMatchedRules', 'testMatchOutcome'], ['onRuleMatchedDebug'],
      { MAX_NUMBER_OF_RULES: 300000, MAX_NUMBER_OF_DYNAMIC_RULES: 30000, MAX_NUMBER_OF_SESSION_RULES: 5000, GUARANTEED_MINIMUM_STATIC_RULES: 30000,
        MAX_NUMBER_OF_STATIC_RULESETS: 100, MAX_NUMBER_OF_ENABLED_STATIC_RULESETS: 50, MAX_NUMBER_OF_REGEX_RULES: 1000,
        DYNAMIC_RULESET_ID: '_dynamic', SESSION_RULESET_ID: '_session' });
    if (has.cookies) chromeApi.cookies = namespace('cookies', ['get', 'getAll', 'set', 'remove', 'getAllCookieStores'], ['onChanged']);
    if (has.downloads) chromeApi.downloads = namespace('downloads', ['download', 'search', 'pause', 'resume', 'cancel', 'open', 'show',
      'showDefaultFolder', 'erase', 'removeFile', 'getFileIcon'], ['onCreated', 'onChanged', 'onErased', 'onDeterminingFilename']);
    if (has.bookmarks) chromeApi.bookmarks = namespace('bookmarks', ['get', 'getChildren', 'getRecent', 'getTree', 'getSubTree', 'search',
      'create', 'move', 'update', 'remove', 'removeTree'], ['onCreated', 'onRemoved', 'onChanged', 'onMoved', 'onChildrenReordered']);
    if (has.history) chromeApi.history = namespace('history', ['search', 'getVisits', 'addUrl', 'deleteUrl', 'deleteRange', 'deleteAll'],
      ['onVisited', 'onVisitRemoved']);
    if (manifest.commands) chromeApi.commands = namespace('commands', ['getAll'], ['onCommand']);
    if (has.idle) chromeApi.idle = namespace('idle', ['queryState', 'getAutoLockDelay'], ['onStateChanged'], {
      setDetectionInterval: function () {}
    });
    if (has.offscreen) chromeApi.offscreen = namespace('offscreen', ['createDocument', 'closeDocument', 'hasDocument'], [],
      { Reason: { TESTING: 'TESTING', AUDIO_PLAYBACK: 'AUDIO_PLAYBACK', IFRAME_SCRIPTING: 'IFRAME_SCRIPTING', DOM_SCRAPING: 'DOM_SCRAPING',
        BLOBS: 'BLOBS', DOM_PARSER: 'DOM_PARSER', USER_MEDIA: 'USER_MEDIA', DISPLAY_MEDIA: 'DISPLAY_MEDIA', WEB_RTC: 'WEB_RTC',
        CLIPBOARD: 'CLIPBOARD', LOCAL_STORAGE: 'LOCAL_STORAGE', WORKERS: 'WORKERS', MATCH_MEDIA: 'MATCH_MEDIA', GEOLOCATION: 'GEOLOCATION' } });
    chromeApi.management = namespace('management', ['getSelf', 'getAll', 'get', 'getPermissionWarningsById', 'setEnabled', 'uninstallSelf'],
      ['onInstalled', 'onUninstalled', 'onEnabled', 'onDisabled']);
  }

  chromeApi.dom = { openOrClosedShadowRoot: function (element) { return element.openOrClosedShadowRoot ? element.openOrClosedShadowRoot() : element.shadowRoot; } };

  // Firefox's `browser`: the same, with promises only (methods return them whatever they are
  // given), and its own names beside Chrome's.
  function promised(ns) {
    var out = {};
    Object.keys(ns).forEach(function (k) {
      var v = ns[k];
      if (typeof v === 'function') out[k] = v;
      else if (v && typeof v === 'object' && !v.addListener && !Array.isArray(v) && k !== 'lastError') out[k] = promised(v);
      else Object.defineProperty(out, k, Object.getOwnPropertyDescriptor(ns, k));
    });
    return out;
  }

  var self_ = {
    deliver: deliver,
    frameId: frameId,
    kind: config.kind,
    id: config.id
  };
  Object.defineProperty(globalThis, '__spatimeshExt', { value: self_, configurable: false, enumerable: false, writable: false });
  if (globalThis.chrome && typeof globalThis.chrome === 'object' && !isContent) {
    // A page of the extension's that the engine gave a `chrome` of its own: ours replaces it.
    try { delete globalThis.chrome; } catch (e) {}
  }
  globalThis.chrome = chromeApi;
  if (config.flavor === 'firefox') {
    var b = promised(chromeApi);
    Object.defineProperty(b.runtime, 'lastError', { get: function () { lastErrorChecked = true; return lastError || undefined; } });
    if (chromeApi.contextMenus && !b.menus) b.menus = b.contextMenus;
    globalThis.browser = b;
  }
  window.addEventListener('pagehide', function () { say('gone', []); });
  say('ready', [config.kind]);
}
