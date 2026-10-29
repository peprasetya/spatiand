// webauthn.js in a real browser: headless Chrome loads pages/webauthn.html with --dump-dom, in
// polyfill mode (the browser's own WebAuthn taken away first, as in SpatiMesh) and in wrapper
// mode (the browser's own kept, as in the extensions), with a stand-in relay answering canned
// replies. The checks (pages/webauthn-checks.js) cover instanceof, the getters, toJSON, the
// errors' names, aborts and timeouts.
//
//   node --test extensions/spatikey/test/*.test.js
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const { serve, dumpDom, chromeThere, startFirefox, CHROME, FIREFOXES } = require('./harness');

const skip = chromeThere() ? false : `no Chrome at ${CHROME}`;

// `promise`, or null once `ms` have passed; the timer goes either way.
function within(promise, ms) {
  let timer;
  const late = new Promise((resolve) => { timer = setTimeout(() => resolve(null), ms); });
  return Promise.race([promise, late]).finally(() => clearTimeout(timer));
}

function fromDom(stdout) {
  const found = /<pre id="results">([\s\S]*?)<\/pre>/.exec(stdout);
  if (!found) return null;
  const text = found[1].replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&#39;/g, '\'').replace(/&amp;/g, '&');
  try {
    return JSON.parse(text);
  } catch (e) {
    return null;
  }
}

for (const mode of ['polyfill', 'wrapper']) {
  test(`webauthn.js in headless Chrome, ${mode} mode`, { skip, timeout: 120000 }, async (t) => {
    const server = await serve();
    try {
      const run = `${mode}-${Date.now()}`;
      const posted = server.results(run);
      const page = await dumpDom(server.url(`/pages/webauthn.html?mode=${mode}&run=${run}`));
      const dumped = fromDom(page.stdout);
      assert.ok(dumped, `no results in the page Chrome dumped (exit ${JSON.stringify(page.exit)}):\n${page.stdout.slice(0, 2000)}\n${page.stderr.slice(-2000)}`);
      const sent = await within(posted, 1000);
      assert.ok(sent && Array.isArray(sent.results), 'the page did not send its results');
      assert.equal(sent.mode, mode);
      assert.deepEqual(dumped, sent.results, 'what the page showed is what it sent');
      const failed = dumped.filter((r) => !r.ok);
      assert.ok(dumped.length >= 60, `only ${dumped.length} checks ran`);
      assert.deepEqual(failed, [], failed.map((r) => `${r.name}: ${r.detail}`).join('\n'));
      t.diagnostic(`${dumped.length} checks passed`);
    } finally {
      await server.close();
    }
  });
}

// The same page in Firefox, which has no --dump-dom: the results come back only as the page
// sends them.
for (const [name, binary] of Object.entries(FIREFOXES)) {
  for (const mode of ['polyfill', 'wrapper']) {
    test(`webauthn.js in headless ${name}, ${mode} mode`, { skip: fs.existsSync(binary) ? false : `no ${name}`, timeout: 120000 }, async (t) => {
      const server = await serve();
      const run = `${name.replace(/\W+/g, '-')}-${mode}-${Date.now()}`;
      const firefox = startFirefox(binary, server.url(`/pages/webauthn.html?mode=${mode}&run=${run}`));
      try {
        const sent = await within(server.results(run), 90000);
        assert.ok(sent && Array.isArray(sent.results), `${name} sent no results`);
        assert.equal(sent.mode, mode);
        const failed = sent.results.filter((r) => !r.ok);
        assert.ok(sent.results.length >= 60, `only ${sent.results.length} checks ran`);
        assert.deepEqual(failed, [], failed.map((r) => `${r.name}: ${r.detail}`).join('\n'));
        t.diagnostic(`${sent.results.length} checks passed`);
      } finally {
        await firefox.stop();
        await server.close();
      }
    });
  }
}
