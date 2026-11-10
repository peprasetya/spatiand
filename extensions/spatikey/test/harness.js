// What the browser tests share: a small server for the test pages and the scripts under test, a
// throwaway browser profile, and Chrome started and always stopped again.
//
// Pages are served from http://localhost:<port>, which browsers count as a secure context and the
// extension's content scripts match. A page's checks send their results to POST /results?<run>;
// GET /hold?<run> answers only once they are in, so that a page holding a picture from it does
// not finish loading -- which is what --dump-dom waits for -- before its checks are done.
'use strict';
const fs = require('node:fs');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');
const { spawn, spawnSync } = require('node:child_process');

const ROOT = path.join(__dirname, '..');
const REPO = path.join(ROOT, '..', '..');
const CHROME = process.env.SPATIKEY_CHROME || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';

const FOLDERS = {
  src: path.join(ROOT, 'src'),
  pages: path.join(__dirname, 'pages')
};
const TYPES = { '.js': 'text/javascript; charset=utf-8', '.html': 'text/html; charset=utf-8', '.json': 'application/json' };

function serve() {
  const results = new Map();
  const waiting = new Map();
  function arrived(run) {
    return results.has(run) ? Promise.resolve(results.get(run)) : new Promise((resolve) => {
      if (!waiting.has(run)) waiting.set(run, []);
      waiting.get(run).push(resolve);
    });
  }
  const server = http.createServer((req, res) => {
    const url = new URL(req.url, 'http://localhost');
    const run = url.search.slice(1);
    if (req.method === 'POST' && url.pathname === '/results') {
      let body = '';
      req.on('data', (chunk) => { body += chunk; });
      req.on('end', () => {
        let parsed;
        try {
          parsed = JSON.parse(body);
        } catch (e) {
          parsed = { error: 'unreadable results: ' + body.slice(0, 200) };
        }
        results.set(run, parsed);
        (waiting.get(run) || []).forEach((fn) => fn(parsed));
        waiting.delete(run);
        res.writeHead(204, { 'access-control-allow-origin': '*' });
        res.end();
      });
      return;
    }
    if (url.pathname === '/hold') {
      const timer = setTimeout(() => { res.writeHead(204); res.end(); }, 45000);
      arrived(run).then(() => { clearTimeout(timer); res.writeHead(204); res.end(); });
      return;
    }
    const parts = url.pathname.split('/').filter(Boolean);
    const folder = FOLDERS[parts[0]];
    const name = parts.length === 2 ? path.basename(parts[1]) : '';
    const file = folder && name ? path.join(folder, name) : '';
    if (!file || !fs.existsSync(file)) {
      res.writeHead(404);
      res.end('not here');
      return;
    }
    res.writeHead(200, { 'content-type': TYPES[path.extname(file)] || 'application/octet-stream', 'cache-control': 'no-store' });
    res.end(fs.readFileSync(file));
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const port = server.address().port;
      resolve({
        port,
        url: (p, host) => `http://${host || 'localhost'}:${port}${p}`,
        results: arrived,
        close: () => new Promise((done) => { server.closeAllConnections(); server.close(done); })
      });
    });
  });
}

function tempDir(name) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `spatikey-${name}-`));
}

// The flags every run gets: a profile of its own, and none of what a browser does by itself on a
// first start (fetching, syncing, asking for the keychain).
function chromeFlags(profile) {
  return [
    '--headless=new',
    `--user-data-dir=${profile}`,
    '--no-first-run',
    '--no-default-browser-check',
    '--disable-background-networking',
    '--disable-component-update',
    '--disable-default-apps',
    '--disable-sync',
    '--disable-features=Translate,OptimizationHints,MediaRouter,DialMediaRouteProvider',
    '--password-store=basic',
    '--use-mock-keychain',
    '--mute-audio'
  ];
}

// End every process still started with `profile` (its folder's name is this run's alone): a
// browser may start itself again behind the process that was started (Firefox does, on a new
// profile), and that one must not outlive the test.
async function endAll(profile) {
  const name = path.basename(profile);
  for (let i = 0; i < 100; i++) {
    if (spawnSync('pgrep', ['-f', name]).status !== 0) return;
    spawnSync('pkill', [i < 50 ? '-TERM' : '-KILL', '-f', name]);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`processes of ${profile} would not end`);
}

function chromeThere() {
  return fs.existsSync(CHROME);
}

// Start Chrome; `stop()` kills it, waits for it to go, and removes its profile.
function startChrome(args, options = {}) {
  const profile = options.profile || tempDir('chrome');
  const child = spawn(CHROME, chromeFlags(profile).concat(args), {
    stdio: options.stdio || ['ignore', 'pipe', 'pipe'],
    env: Object.assign({}, process.env, options.env || {})
  });
  let exited = false;
  const gone = new Promise((resolve) => child.on('exit', (code, signal) => { exited = true; resolve({ code, signal }); }));
  return {
    child,
    profile,
    gone,
    async stop() {
      if (!exited) {
        child.kill('SIGTERM');
        const late = setTimeout(() => { if (!exited) child.kill('SIGKILL'); }, 3000);
        await gone;
        clearTimeout(late);
      }
      await endAll(profile);
      fs.rmSync(profile, { recursive: true, force: true });
    }
  };
}

// Load `url` with --dump-dom: what Chrome printed of the page once it had loaded.
async function dumpDom(url, timeoutMs = 60000) {
  const chrome = startChrome(['--dump-dom', url]);
  let stdout = '', stderr = '';
  chrome.child.stdout.on('data', (chunk) => { stdout += chunk; });
  chrome.child.stderr.on('data', (chunk) => { stderr += chunk; });
  const timer = setTimeout(() => chrome.child.kill('SIGKILL'), timeoutMs);
  try {
    const exit = await chrome.gone;
    return { stdout, stderr, exit };
  } finally {
    clearTimeout(timer);
    await chrome.stop();
  }
}

// Firefox, headless, on a profile of its own that asks nothing of the network on its own.
const FIREFOXES = {
  Firefox: process.env.SPATIKEY_FIREFOX || '/Applications/Firefox.app/Contents/MacOS/firefox',
  'Firefox Developer Edition': '/Applications/Firefox Developer Edition.app/Contents/MacOS/firefox'
};
const FIREFOX_PREFS = [
  ['app.normandy.enabled', false], ['app.update.disabledForTesting', true], ['browser.aboutwelcome.enabled', false],
  ['browser.safebrowsing.malware.enabled', false], ['browser.safebrowsing.phishing.enabled', false], ['browser.safebrowsing.downloads.enabled', false],
  ['browser.shell.checkDefaultBrowser', false], ['browser.startup.homepage_override.mstone', 'ignore'], ['browser.startup.page', 0],
  ['datareporting.healthreport.uploadEnabled', false], ['datareporting.policy.dataSubmissionEnabled', false], ['extensions.update.enabled', false],
  ['network.captive-portal-service.enabled', false], ['network.connectivity-service.enabled', false], ['toolkit.telemetry.reportingpolicy.firstRun', false],
  ['browser.search.update', false], ['services.settings.server', 'http://127.0.0.1:9/']
];

function startFirefox(binary, url) {
  const profile = tempDir('firefox');
  fs.writeFileSync(path.join(profile, 'user.js'), FIREFOX_PREFS.map(([k, v]) => `user_pref(${JSON.stringify(k)}, ${JSON.stringify(v)});`).join('\n') + '\n');
  const child = spawn(binary, ['--headless', '--no-remote', '--profile', profile, url], { stdio: ['ignore', 'ignore', 'pipe'] });
  let exited = false;
  const gone = new Promise((resolve) => child.on('exit', () => { exited = true; resolve(); }));
  return {
    child,
    async stop() {
      if (!exited) {
        child.kill('SIGTERM');
        const late = setTimeout(() => { if (!exited) child.kill('SIGKILL'); }, 5000);
        await gone;
        clearTimeout(late);
      }
      await endAll(profile);
      fs.rmSync(profile, { recursive: true, force: true });
    }
  };
}

module.exports = { ROOT, REPO, CHROME, FIREFOXES, serve, tempDir, startChrome, startFirefox, chromeThere, dumpDom };
