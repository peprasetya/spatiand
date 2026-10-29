# SpatiKey for Chrome and Firefox

The SpatiKey extension gives Chrome, Chromium, Brave, Edge and Firefox the passkeys and passwords
kept in SpatiKey, Spatiand's own manager (docs/spatikey.md). Every use is approved with Touch ID on
the Mac -- on the Mac itself, or from the Deck or the Beam Pro while the Mac is on the same network.
It is installed the developer way and not published.

## Build

```sh
extensions/spatikey/build.sh
```

makes `dist/spatikey-chrome` (Manifest V3, for the Chromium family), `dist/spatikey-firefox`
(Manifest V3 for Firefox 128 and later) and `dist/spatikey-firefox.xpi`. Nothing is fetched and
nothing needs installing; `dist/` is not kept in git.

## Install in Chrome, Chromium, Brave or Edge

1. Open `chrome://extensions` (`brave://extensions`, `edge://extensions`), turn on **Developer
   mode**, choose **Load unpacked** and pick `extensions/spatikey/dist/spatikey-chrome`.
2. Its id is **`ndebkmflcohflpgdclfbnjjeogbigpib`**, the same everywhere: the manifest carries a
   fixed public key, and Chrome makes the id from it. The native-messaging host manifest names
   this id, so it must not change.
3. Tell the browsers where SpatiKey's host is:

   ```sh
   tools/spatikey-browsers.sh --print   # what it would write
   tools/spatikey-browsers.sh           # write it, for every browser that is here
   ```

   The host is `spatikey-host`: inside Spatiand.app on the Mac
   (`~/Applications/Spatiand.app/Contents/MacOS/spatikey-host`), beside `spatiand` on Linux. Give
   another path as the script's argument. `--remove` takes the manifests away again.

After `build.sh` again, press the reload arrow on the extension's card. The background's console
is the **service worker** link on that card; the page's half logs to the page's own console.

## Install in Firefox

Firefox 128 or later: the page's half runs as a `"world": "MAIN"` content script, which Firefox
has only since 128. (The Firefox Developer Edition on this Mac is 123: update it first.)

- **For a session:** `about:debugging#/runtime/this-firefox` → **Load Temporary Add-on…** → pick
  `dist/spatikey-firefox/manifest.json`. It is gone when Firefox quits.
- **For good:** Firefox Developer Edition (or Nightly) with `xpinstall.signatures.required` set to
  `false` in `about:config`, then `about:addons` → the gear → **Install Add-on From File…** →
  `dist/spatikey-firefox.xpi`. Release Firefox installs only signed add-ons for good.

Its id is `spatikey@prasetya.id`; `tools/spatikey-browsers.sh` writes Firefox's host manifest too.
If Firefox lists the site access as a permission it has not been given, allow "Access your data for
all websites" in `about:addons` → SpatiKey → Permissions.

## What it does

- **Passkeys.** A site's `navigator.credentials.create()` and `get()` go to SpatiKey first. When
  SpatiKey has nothing for the site, cannot be asked (not installed, not set up, no Mac near) or the
  person picks "Use the browser's own" in the chooser, the browser's own WebAuthn answers instead,
  as if SpatiKey were not there. What SpatiKey cannot do at all -- security keys, algorithms other
  than ES256, PRF, large blobs -- goes straight to the browser's own. A site that waits for a
  passkey in its login field (`mediation: "conditional"`) gets SpatiKey's when the key button in
  the field is pressed, and the browser's own autofill keeps working beside it.
- **Filling.** The key button inside a login field fills it from SpatiKey: the account chosen,
  when the site has several.
- **Saving.** A login typed and sent on a page is offered for saving in a small bar ("Save in
  SpatiKey?"), also on the page the login leads to.

The origin SpatiKey is asked for is always the browser's own word (`sender.origin`), and
clientDataJSON is built from it exactly as browsers build it; a page cannot say it is another site.
Frames of another site inside a page are left to the browser's own WebAuthn, which knows their
permissions policy. Pages that are not https (or this computer's own, `localhost`) are left alone.

Do not install it in SpatiMesh, which has SpatiKey built in.

## The files

| | |
|---|---|
| `src/webauthn.js` | the page's half, in the page's own world: wraps the browser's WebAuthn (or, in SpatiMesh, which has none, defines it), and speaks only through events on the document |
| `src/forms.js` | finds login fields, fills them, notices logins being sent, draws the key button; shared with SpatiMesh |
| `src/content.js` | the isolated world's relay to the background, the chooser and the save bar |
| `src/background.js` | the native-messaging connection to `spatikey-host`, the origins, clientDataJSON, the choosing |
| `manifest.chrome.json`, `manifest.firefox.json` | the manifests, `@VERSION@` filled in by `build.sh` |
| `icons/` | the icons, and `make-icons.js`, which draws them |

SpatiMesh uses `webauthn.js` and `forms.js` as they are, with its own relay,
`crates/spatimesh/src/credentials/cred-world.js`, in place of `content.js` and `background.js`.

## Tests

```sh
node --test extensions/spatikey/test/*.test.js
```

- `webauthn.test.js`, `background.test.js`: in node alone -- base64url, what the page's options
  become as requests, what replies become as credentials, errors, aborts, timeouts; origins,
  clientDataJSON and its hash, the service's requests, choosing, filling and saving.
- `browser.test.js`: `webauthn.js` in headless Chrome (`--dump-dom`) and headless Firefox, with
  the browser's own WebAuthn taken away (SpatiMesh's case) and kept (the extensions').
- `spatimesh.test.js`: `cred-world.js` and `forms.js` in an isolated world of headless Chrome,
  with the test playing SpatiMesh's window; typing and clicking as a person does.
- `extension.test.js`: the built Chrome extension, whole, with a stand-in for `spatikey-host`
  (`fake-host.js`) and a virtual authenticator as the browser's own.
- `firefox-extension.test.js`: the Firefox build as a temporary add-on in headless Firefox (over
  WebDriver BiDi), without a host: Firefox reads host manifests only from the user's own Mozilla
  folder, which a test must not touch. It checks the add-on is taken, its scripts run, and the key
  button reaches the background, which says the helper is not there.
- `browsers-script.test.js`: `tools/spatikey-browsers.sh`, run with `HOME` in a throwaway folder.

The browser tests need Google Chrome (and Firefox, which they skip without) in `/Applications`, or
`SPATIKEY_CHROME` / `SPATIKEY_FIREFOX` naming them. They run on throwaway profiles in the system's
temporary folder, register the stand-in host only inside such a profile, and stop every browser they
start.

## The key

The manifest's `key` is the public half of an RSA key made for this; the private half was not kept.
It only fixes the id of the unpacked extension; publishing would mean a new key and a new id. The id
is the first 32 hex digits of the key's SHA-256, written with the letters a-p:

```sh
node -e 'const k = require("./extensions/spatikey/manifest.chrome.json").key; const h = require("crypto").createHash("sha256").update(Buffer.from(k, "base64")).digest("hex").slice(0, 32); console.log([...h].map((c) => String.fromCharCode(97 + parseInt(c, 16))).join(""))'
```
