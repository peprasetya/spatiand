// forms.js's choice of which password a sent form carries, in node alone: the rest of forms.js
// needs a page (spatimesh.test.js, extension.test.js).
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const source = fs.readFileSync(path.join(__dirname, '..', 'src', 'forms.js'), 'utf8');
const { passwordAmong } = new Function(source + '\nreturn SpatiKeyForms;')();
const typed = (value, kind) => ({ field: {}, value, kind: kind || 'password' });
const chosen = (list) => { const c = passwordAmong(list); return c && [c.value, c.kind]; };

test('a sign-in: the one password typed', () => {
  assert.equal(passwordAmong([]), null);
  assert.deepEqual(chosen([typed('hunter2')]), ['hunter2', 'password']);
});

test('a field marked as new is the new password', () => {
  assert.deepEqual(chosen([typed('old'), typed('new', 'new-password'), typed('new', 'new-password')]), ['new', 'new-password']);
});

test('a change of password with nothing marked: the new one, not the old', () => {
  // Current, new, confirmation.
  assert.deepEqual(chosen([typed('old'), typed('new'), typed('new')]), ['new', 'new-password']);
  // Current and new, no confirmation.
  assert.deepEqual(chosen([typed('old'), typed('new')]), ['new', 'new-password']);
  // The confirmation mistyped: still not the old one.
  assert.deepEqual(chosen([typed('old'), typed('new'), typed('nwe')]), ['nwe', 'new-password']);
  // The current one filled by SpatiKey is not among them: new and its confirmation.
  assert.deepEqual(chosen([typed('new'), typed('new')]), ['new', 'new-password']);
});

test('the field chosen is the one whose value it is', () => {
  const list = [typed('old'), typed('new'), typed('new')];
  assert.equal(passwordAmong(list).field, list[1].field);
});
