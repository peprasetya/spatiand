// A stand-in for spatikey-host and the SpatiKey service behind it, for extension.test.js. Chrome
// starts it through the native-messaging manifest the test writes into its throwaway profile, and
// speaks native messaging to it: a 4-byte length in this machine's byte order, then JSON. It
// writes every request down (one JSON line each, to $SPATIKEY_FAKE_LOG) and answers from a
// script of canned replies, in the service's protocol (crates/spatikey-core/src/wire.rs).
'use strict';
const fs = require('node:fs');

const LOG = process.env.SPATIKEY_FAKE_LOG;
const b64 = (text) => Buffer.from(text).toString('base64url');

function choice(name, n) {
  return { user_name: name, user_display: name.toUpperCase(), credential_id: b64(`spatikey-${name}`), authenticator_data: b64(`ad-${n}`), signature: b64(`sig-${n}`), user_handle: b64(`uh-${n}`) };
}

function answer(request) {
  switch (request.op) {
    case 'passkey_create':
      // SpatiKey not set up: the page gets the browser's own.
      if (request.user_name === 'use-the-browser') return { err: { code: 'no_vault', message: 'There is no vault yet.' } };
      if (request.user_name === 'taken') return { err: { code: 'excluded', message: 'Already here.' } };
      return { ok: { credential_id: b64('spatikey-made'), authenticator_data: b64('auth-data'), attestation_object: b64('attestation'), public_key: b64('spki'), alg: -7, transports: ['hybrid', 'internal'] } };
    case 'passkey_get':
      // Credentials named by the site are the browser's own (made through it): none of SpatiKey's.
      if (request.allow.length) return { err: { code: 'no_match', message: '' } };
      return { ok: { choices: request.conditional ? [choice('paulus', 1)] : [choice('paulus', 1), choice('work', 2)] } };
    case 'fill':
      return { ok: { logins: [
        { id: 'e1', title: 'Personal', username: 'paulus', password: 'first-pw', totp: null },
        { id: 'e2', title: 'Work', username: 'work@example.com', password: 'second-pw', totp: '123456' }
      ] } };
    case 'save':
      return { ok: { id: 'saved-1' } };
    default:
      return { err: { code: 'bad_request', message: `no ${request.op} here` } };
  }
}

function send(message) {
  const body = Buffer.from(JSON.stringify(message));
  const head = Buffer.alloc(4);
  head.writeUInt32LE(body.length, 0);
  process.stdout.write(Buffer.concat([head, body]));
}

let pending = Buffer.alloc(0);
process.stdin.on('data', (chunk) => {
  pending = Buffer.concat([pending, chunk]);
  while (pending.length >= 4) {
    const length = pending.readUInt32LE(0);
    if (pending.length < 4 + length) break;
    const request = JSON.parse(pending.subarray(4, 4 + length).toString('utf8'));
    pending = pending.subarray(4 + length);
    if (LOG) fs.appendFileSync(LOG, JSON.stringify(request) + '\n');
    send(Object.assign({ id: request.id }, answer(request)));
  }
});
process.stdin.on('end', () => process.exit(0));
