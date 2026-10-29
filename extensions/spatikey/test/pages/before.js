// Run before webauthn.js (webauthn.html). Polyfill mode: the browser's own WebAuthn goes, as
// SpatiMesh's engine has none. Wrapper mode: its create() and get() become stand-ins that say
// they were called. Both: long timers are made short while webauthn.js keeps its own copy of
// setTimeout, so that a timeout can be watched in milliseconds rather than half a minute.
(function () {
  'use strict';
  var mode = new URLSearchParams(location.search).get('mode');
  window.testMode = mode;
  if (mode === 'polyfill') {
    ['PublicKeyCredential', 'Credential', 'PasswordCredential', 'FederatedCredential', 'IdentityCredential', 'OTPCredential',
      'DigitalCredential', 'CredentialsContainer', 'AuthenticatorResponse', 'AuthenticatorAttestationResponse',
      'AuthenticatorAssertionResponse'].forEach(function (name) { delete window[name]; });
    delete Navigator.prototype.credentials;
  } else {
    window.nativeCalls = [];
    var proto = CredentialsContainer.prototype;
    proto.create = function create(options) {
      window.nativeCalls.push({ op: 'create', options: options, self: this });
      return Promise.resolve('native create');
    };
    proto.get = function get(options) {
      window.nativeCalls.push({ op: 'get', options: options, self: this });
      if (options && options.mediation === 'conditional') {
        return new Promise(function (resolve, reject) {
          window.nativeConditional = resolve;
          if (options.signal) options.signal.addEventListener('abort', function () { reject(options.signal.reason); });
        });
      }
      return Promise.resolve('native get');
    };
  }
  var realSetTimeout = window.setTimeout;
  window.realSetTimeout = realSetTimeout;
  window.setTimeout = function (fn, ms) { return realSetTimeout(fn, ms >= 30000 ? 30 : ms); };
})();
