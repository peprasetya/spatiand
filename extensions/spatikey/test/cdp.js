// Just enough of the Chrome DevTools Protocol for the tests that need more than --dump-dom: typing
// and clicking as a person does (events the page sees as trusted), scripts in isolated worlds,
// loading the extension, a virtual authenticator. Spoken over --remote-debugging-pipe (Chrome reads
// on its fd 3 and writes on its fd 4, each message JSON ending in a NUL), so no port is opened.
'use strict';
const { startChrome } = require('./harness');

class Cdp {
  constructor(child) {
    this.out = child.stdio[3];
    this.next = 1;
    this.calls = new Map();
    this.listeners = new Set();
    let pending = Buffer.alloc(0);
    child.stdio[4].on('data', (chunk) => {
      pending = Buffer.concat([pending, chunk]);
      let at;
      while ((at = pending.indexOf(0)) >= 0) {
        const message = JSON.parse(pending.subarray(0, at).toString('utf8'));
        pending = pending.subarray(at + 1);
        this.receive(message);
      }
    });
  }

  receive(message) {
    const call = message.id ? this.calls.get(message.id) : null;
    if (call) {
      this.calls.delete(message.id);
      if (message.error) call.reject(new Error(`${call.method}: ${message.error.message}`));
      else call.resolve(message.result);
      return;
    }
    for (const fn of [...this.listeners]) fn(message);
  }

  send(method, params = {}, sessionId) {
    const id = this.next++;
    const message = { id, method, params };
    if (sessionId) message.sessionId = sessionId;
    return new Promise((resolve, reject) => {
      this.calls.set(id, { resolve, reject, method });
      this.out.write(JSON.stringify(message) + '\0');
    });
  }

  on(fn) {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  // The first event (or one already kept by `seen`) that `match` takes.
  waitFor(match, what, ms = 10000) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { off(); reject(new Error(`waited ${ms} ms for ${what}`)); }, ms);
      const off = this.on((message) => {
        if (!match(message)) return;
        clearTimeout(timer);
        off();
        resolve(message);
      });
    });
  }
}

// Chrome with the protocol on a pipe. `stop()` ends it and removes its profile.
async function launch(extraArgs = [], options = {}) {
  const chrome = startChrome(['--remote-debugging-pipe', '--window-size=1200,900'].concat(extraArgs), Object.assign({ stdio: ['ignore', 'ignore', 'pipe', 'pipe', 'pipe'] }, options));
  let stderr = '';
  chrome.child.stderr.on('data', (chunk) => { stderr = (stderr + chunk).slice(-4000); });
  const cdp = new Cdp(chrome.child);
  await cdp.send('Browser.getVersion');
  return { cdp, chrome, stderr: () => stderr, stop: () => chrome.stop() };
}

// A tab, attached: `send` speaks to it, and the helpers act in it as a person would.
async function openTab(cdp) {
  const { targetId } = await cdp.send('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await cdp.send('Target.attachToTarget', { targetId, flatten: true });
  const send = (method, params) => cdp.send(method, params, sessionId);
  const tab = {
    targetId,
    sessionId,
    send,
    // An event of this tab.
    waitFor: (method, match, what, ms) => cdp.waitFor((m) => m.sessionId === sessionId && m.method === method && (!match || match(m.params)), what || method, ms),
    async evaluate(expression, contextId) {
      const params = { expression, awaitPromise: true, returnByValue: true };
      if (contextId) params.contextId = contextId;
      const { result, exceptionDetails } = await send('Runtime.evaluate', params);
      if (exceptionDetails) throw new Error(`${expression.slice(0, 80)}: ${exceptionDetails.exception ? exceptionDetails.exception.description : exceptionDetails.text}`);
      return result.value;
    },
    async click(x, y) {
      await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y });
      await send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', clickCount: 1 });
      await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', clickCount: 1 });
    },
    // Click the middle of what `selector` finds in the page.
    async clickOn(selector) {
      const rect = await tab.evaluate(`(() => { const r = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; })()`);
      await tab.click(rect.x, rect.y);
    },
    type: (text) => send('Input.insertText', { text }),
    async key(key) {
      const codes = { Enter: 13, ArrowDown: 40, ArrowUp: 38, Escape: 27, Tab: 9 };
      const down = { type: 'keyDown', key, code: key, windowsVirtualKeyCode: codes[key], nativeVirtualKeyCode: codes[key] };
      if (key === 'Enter') down.text = '\r';
      await send('Input.dispatchKeyEvent', down);
      await send('Input.dispatchKeyEvent', { type: 'keyUp', key, code: key, windowsVirtualKeyCode: codes[key], nativeVirtualKeyCode: codes[key] });
    },
    async navigate(url) {
      const loaded = tab.waitFor('Page.loadEventFired', null, `loading ${url}`, 15000);
      await send('Page.navigate', { url });
      await loaded;
    }
  };
  return tab;
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// Poll `fn` until it gives something truthy.
async function until(fn, what, ms = 10000) {
  const end = Date.now() + ms;
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() > end) throw new Error(`waited ${ms} ms for ${what}`);
    await sleep(50);
  }
}

module.exports = { launch, openTab, sleep, until };
