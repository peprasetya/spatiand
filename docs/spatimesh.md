# SpatiMesh, Spatiand's web browser

A full browser in the class of Safari and Chrome, written as one of Spatiand's own applications
(docs/spatiand-apps.md): a window in the room drawn with the SDK and the room's widgets, the same
program on the Mac, the Deck, other Linux and the Beam Pro. The page itself is drawn by the
platform's own web engine, behind one Rust interface. Asked for on 2026-10-16; this is the plan,
and at its end what has been built of it.

| Platform | Engine | Where it comes from |
|---|---|---|
| Mac | WebKit (`WKWebView`) | macOS |
| SteamOS (the Deck) | **Steam's Chromium** (CEF, what `steamwebhelper` runs on) | the Steam client, `~/.local/share/Steam/ubuntu12_64/libcef.so`, loaded at run time |
| Other Linux | WebKitGTK (`webkit2gtk-4.1`) | the distribution's package |
| Android (Beam Pro) | the system WebView | Android |

**Nothing is bundled and nothing is installed beside Spatiand for it: no Flatpak, no engine of
our own.** Spatiand stays one thing to install; SpatiMesh uses what each system already has.

## Three kinds of process

```text
spatimesh                    the window process: every SpatiMesh window, the tab bar, the address
                             bar, bookmarks, profiles, settings. No engine in it. Draws each tab's
                             picture into its window and passes the wearer's input on.
spatimesh --engine <profile> the engine host, one per profile in use (the private one has its
                             own): the engine's own "UI process" half -- WKWebView, CEF,
                             WebKitGTK -- which owns the tabs' web views, draws each into shared
                             memory and takes input for it. Android: a Java service in its own
                             process.
<the engine's processes>     started by the engine host, not by us: each tab's page in its own
                             renderer (content) process; networking and GPU processes shared.
```

**Why a host per profile and not per tab.** A profile's cookies and site storage are one data
store, kept by one networking process. Two engine instances in two processes over the same store
would each keep their own copy of it in memory (a login made in one tab unseen in the next), and
Android refuses outright (`WebView.setDataDirectorySuffix`: one process per data directory). So
per-tab isolation is left to the engine's own multi-process, which is exactly Safari's and
Chrome's model, and our own processes separate the window from the engines and the profiles
from each other.

What a crash takes with it:

| What crashes | Mac (WKWebView) | SteamOS (CEF) | Linux (WebKitGTK) | Android (WebView) |
|---|---|---|---|---|
| A page's own process | that tab only: "this page stopped", reload | that tab (Chromium's renderer per site) | that tab only | **every tab of that profile**: one renderer serves all of an app process's WebViews; each gets `onRenderProcessGone` and is reloaded |
| The engine's network/GPU process | relaunched by WebKit | relaunched by Chromium | relaunched by WebKit | (inside the app process) |
| The engine host | that profile's tabs; the windows stay and reopen them | same | same | same (the service process) |
| The window process | everything visible; the engine hosts exit with it | same | same | same |

On Android the tabs of one profile are **not** isolated from each other; profiles are (each its
own Android process, `android:process`, and so its own renderer). That is the most Android WebView
allows: it has no process-per-tab mode, and a WebView per process means a data directory per
process, i.e. a cookie jar per tab.

## The engine interface

`crates/spatimesh/src/engine/mod.rs`. One trait, implemented once per platform, living in the
engine host; the window process speaks the same operations over a socket (`wire.rs`), so the
window never links an engine.

```rust
trait Engine {
    fn open(&mut self, tab: TabId, url: &str, view: View) -> Result<(), String>;  // size, zoom
    fn close(&mut self, tab: TabId);
    fn load(&mut self, tab: TabId, url: &str);
    fn back(&mut self, tab: TabId);  fn forward(&mut self, tab: TabId);
    fn reload(&mut self, tab: TabId); fn stop(&mut self, tab: TabId);
    fn resize(&mut self, tab: TabId, view: View);
    fn pointer(&mut self, tab: TabId, at: (f32, f32), button: Option<(Button, bool)>);
    fn scroll(&mut self, tab: TabId, at: (f32, f32), dx: f32, dy: f32);
    fn key(&mut self, tab: TabId, key: &KeyPress);   // evdev code, key, text, modifiers
    fn focus(&mut self, tab: TabId, focused: bool);
    fn set_window(&mut self, tab: TabId, window: u32); // where its sound goes
    fn set_muted(&mut self, tab: TabId, muted: bool);
    fn pump(&mut self, wait: Duration) -> Vec<Happened>;
}
// Happened: Title, Url, Progress, History (can go back/forward), Picture (a frame in shared
// memory, with what changed), Cursor, Sound (playing or not), OpenTab (target=_blank,
// window.open), Crashed.
```

Pictures travel as shared memory (memfd on Linux and Android, POSIX shm on the Mac) handed over
once per size, then a short message per frame naming the buffer and what changed; the window
uploads what changed to a texture. One copy, on every platform alike. (Zero-copy -- dmabuf,
IOSurface, AHardwareBuffer -- is a later step per platform.)

## What each engine can and cannot do

| | Mac: WKWebView | SteamOS: Steam's CEF | Linux: WebKitGTK 4.1 | Android: system WebView |
|---|---|---|---|---|
| A page with no window on screen | offscreen `NSWindow`, occlusion detection off (SPI `_windowOcclusionDetectionEnabled`); frames by `takeSnapshot` | CEF's windowless rendering: `OnPaint` hands each frame with what changed | `GtkOffscreenWindow`, its surface read after each draw | a `VirtualDisplay` + `Presentation` drawing into an `ImageReader` |
| Input | synthesised `NSEvent`s sent to the view | `SendMouseClickEvent`, `SendKeyEvent`, ... | synthesised GDK events | `dispatchTouchEvent`/`dispatchKeyEvent` |
| Tab per process | yes (WebKit's) | yes (Chromium's) | yes (`MULTIPLE_SECONDARY_PROCESSES`) | no (see above) |
| Profiles | `WKWebsiteDataStore(forIdentifier:)` (macOS 14) | a `CefRequestContext` per profile, its own folder | a `WebKitWebsiteDataManager` per profile | a process and data directory per profile |
| Private | `nonPersistentDataStore`: in memory | a request context with no folder: in memory | ephemeral data manager: in memory | a fresh data directory, wiped when the profile's process ends and at the next start: **it does write to disk while open** |
| Each tab's sound | no (see Sound) | **yes**: `CefAudioHandler` hands each tab's samples | yes, by process | no |
| Extensions | see Extensions | see Extensions | see Extensions | see Extensions |

### SteamOS: Steam's Chromium

Decided by the owner (2026-10-16): on SteamOS, Steam's own Chromium; WebKitGTK only on a Linux
that is not SteamOS (told apart by `/etc/os-release`, `ID=steamos`; `SPATIMESH_ENGINE=cef` or
`webkitgtk` overrides). What that means:

* **It is CEF**, the Chromium Embedded Framework, in the Steam client: `libcef.so` with its
  `.pak` files, `icudtl.dat`, `v8_context_snapshot.bin` and `locales/` in
  `~/.local/share/Steam/ubuntu12_64/`. The engine host loads it from there (by
  `LD_LIBRARY_PATH`, set by the window process when it starts the host) and is its own CEF
  subprocess; nothing of Steam's is copied or changed. Steam is always there on SteamOS.
* **CEF's C interface is tied to its version.** A program is built against one CEF version's
  headers and `libcef` checks that (`cef_api_hash`); CEF's *stable API versions*
  ([CEF](https://chromiumembedded.github.io/cef/api_versioning.html)) loosen this to a range of
  versions, but only from CEF 13x on. Steam's on SteamOS was reported as Chromium 126.0.6478.183
  ([steam-for-linux #13679](https://github.com/ValveSoftware/steam-for-linux/issues/13679)), before
  them. So the CEF wrapper is built against the headers of the version Steam ships, checks the
  hash at start, and says so plainly if Steam has moved on (a rebuild against the new headers
  is then needed). A patch of Valve's that changes the C interface would show up there too.
* **Not checkable from here**, to be looked at on the Deck: which CEF Steam has now
  (`strings libcef.so | grep -m1 'Chrome/'`); whether `libcef.so` finds its libraries on the
  SteamOS system itself or only inside Steam's runtime container, which `steamwebhelper` is run
  in (if only there, the host is run the same way, through `SteamLinuxRuntime_sniper`: Steam's
  own, not a Flatpak); whether Chromium's sandbox works for a process that is not Steam's.
* **Tested in the cloud** against the official CEF build of the same version (CEF's own
  distribution, used only by CI): pages drawn windowless, input, each tab's sound.

**As built** (`crates/spatimesh-cef`): C against CEF 126's C interface (`native/cef.c`),
compiled against the headers of CEF's own 126 build (`CEF_ROOT`, from `tools/fetch-cef.sh`);
`libcef.so` is not linked but loaded when the host starts, from `SPATIMESH_CEF_DIR` or Steam's
`ubuntu12_64` (also through `~/.steam/steam` and `~/.steam/root`). Before anything else it
compares the library's interface hash with the headers' and refuses, saying which CEF Steam
has, if they differ. Chromium's own processes are `spatimesh-cef` started again with `--type=`.
Settings: windowless, Chromium's message loop pumped from the host's own loop, drawing in
software (`disable-gpu`: frames are read from memory anyway), the profile's data in its data
folder and its HTTP cache in its cache folder (`disk-cache-dir`), cookies' key kept by
Chromium (`password-store=basic`, no desktop keyring needed in Game Mode), the private profile
in memory with a scratch folder in `$XDG_RUNTIME_DIR` removed at the end. The sandbox for
pages' processes is on unless `SPATIMESH_CEF_NO_SANDBOX=1` (CI sets it: the runner does not
allow Chromium's sandbox). New windows and middle-clicked links become SpatiMesh tabs;
script dialogs are SpatiMesh's; a `<select>` asks SpatiMesh for its list by a script put into
each page when it has loaded (before that, and inside frames, Chromium's own list is drawn
over the page); context menus and file choosers are refused (no window of Chromium's own);
downloads come into the profile's cache and are moved into `~/Downloads` when finished. Not
done yet: each tab's sound (CEF plays it itself for now: not placed in the room). CI runs the
same six tests as WebKitGTK's against CEF's 126; downloads are not among them.

More to check on the Deck: the layout of `cef_settings_t` depends on how a CEF was built
(whether it keeps the "Alloy bootstrap", as CEF's own 126 builds do); a Steam build without it
would pass the hash check and still be read wrongly. Chromium's errors go where the host's own
messages go (Spatiand's applications' log); `SPATIMESH_CEF_LOG=1` makes it say more, and
`SPATIMESH_CEF_OZONE=x11|wayland` picks how it talks to the display.

## Extensions: what each engine allows (findings, before building)

Chrome and Firefox extensions are both WebExtensions; a `.crx` (Chrome) and an `.xpi` (Firefox)
are zips with a manifest. Installing in developer mode is ours on every platform: unpack the
folder, `.zip`, `.crx` or `.xpi`, read the manifest, list it in the profile. What runs it is not:

* **Mac, macOS 15.4 and later: real WebExtensions.** WebKit's own API (`WKWebExtension`,
  `WKWebExtensionContext`, `WKWebExtensionController`), what Safari's extensions run on: Manifest
  V2 and V3, background pages and service workers, content scripts, the toolbar action and its
  popup, `storage`, `tabs`, `windows`, `scripting`, `declarativeNetRequest`, messaging, and the
  rest of what Safari supports. We expose our tabs and windows to it (`WKWebExtensionTab`,
  `WKWebExtensionWindow`) and draw its popups. Not there: what Safari lacks too -- blocking
  `webRequest` (Manifest V2 uBlock Origin; its Lite version works), `debugger`, Chrome-only and
  Firefox-only APIs. **macOS 14 to 15.3: none.**
* **SteamOS (CEF): Chrome's own extension system, with questions.** CEF's Chrome runtime is
  Chrome's code and loads unpacked Chrome extensions (`--load-extension`), Chrome's APIs and all;
  Firefox-only APIs do not exist there. The engine as built runs CEF's other runtime, Alloy,
  which windowless tabs need in CEF 126; the Chrome runtime takes windowless tabs only as
  "Alloy-style" browsers, which Chrome's `tabs` API and the toolbar's popups may not see.
  What would work -- content scripts and background workers most likely, `tabs` and popups
  least -- is to be tried against CEF 126 before building, and whether Valve's build has the
  Chrome runtime at all, on the Deck. CEF's older built-in subset for Alloy
  (`CefRequestContext::LoadExtension`) covers a few APIs only and is gone from later CEF.
* **Other Linux (WebKitGTK): no extension API.** Only a subset we write ourselves on WebKitGTK's
  primitives, as GNOME Web does: content scripts and styles (`WebKitUserScript` and
  `WebKitUserStyleSheet` in an isolated world), messaging (`register_script_message_handler`),
  `storage`, a background page in a hidden view, the action popup, part of `tabs`, and
  `declarativeNetRequest`'s static block/allow rules as WebKit content-blocker rules. No blocking
  `webRequest`, no service-worker backgrounds, no devtools pages. Many extensions will not run.
* **Android (WebView): no extension API.** The same kind of subset on
  `addDocumentStartJavaScript`, `addWebMessageListener` and `shouldInterceptRequest`, with the
  same limits and less.

## Extensions: SpatiMesh's own WebExtensions runtime (decided 2026-10-16)

The owner's decision, given the findings above: Chrome and Firefox extensions are to run on
every engine, and where an engine has no extension system, SpatiMesh is to have one of its own,
modelled on Chromium's and Firefox's (both open: `chrome/browser/extensions`,
`extensions/` and `toolkit/components/extensions` are the references).

**One runtime for every engine.** What an extension is and what its APIs do is written once, in
Rust, in the window process, which owns the tabs, windows and profiles the APIs are about:

```text
crates/spatimesh/src/extensions/
  manifest.rs   manifest.json, Manifest V2 and V3, Chrome's and Firefox's keys (background
                page/scripts/service_worker, action/browser_action/page_action,
                browser_specific_settings, web_accessible_resources, declarative_net_request,
                content_scripts, options_ui, commands), __MSG_x__ and _locales
  install.rs    developer mode: a folder (loaded where it is), a .zip, a .crx (CRX3 header) or
                an .xpi, unpacked into the profile's local folder; ids as Chrome makes them (from
                `key`, or from the folder) and Firefox's gecko id kept as runtime.id's alias
  pattern.rs    match patterns, globs, `<all_urls>`, about:blank and frames
  runtime.rs    the API calls and events: runtime (messages, ports, getURL, onInstalled ...),
                storage (local; sync in the shared folder, so it syncs; session), tabs, windows,
                scripting, action/browserAction/pageAction, i18n, alarms, notifications,
                permissions, contextMenus/menus, webNavigation, cookies, bookmarks (SpatiMesh's),
                downloads, commands
  dnr.rs        declarativeNetRequest's rules, matched here or compiled for an engine
  api.js        `chrome` and `browser` (promises, and callbacks with lastError) in every
                extension context; JSON over the engine's bridge to the window and back
```

**Each engine only bridges**: put a script into a frame at document_start/end/idle in a world of
the extension's own where the engine has worlds, carry JSON from a context to the window and
back into a given tab, frame and world, serve `chrome-extension://<id>/` (and `moz-extension://`)
from the extension's folder, run a background page in a tab nobody sees, and, where it can, see
and stop requests (webRequest, declarativeNetRequest):

| | worlds for content scripts | to the window | resources | requests |
|---|---|---|---|---|
| WebKitGTK | `webkit_user_script_new_for_world` | script message handlers in that world | `webkit_web_context_register_uri_scheme` | content filters (DNR); a web-process extension's `send-request` (blocking webRequest) |
| Mac (WKWebView) | `WKContentWorld` (`WKUserScript`s; WebKit's own patterns where its SPI has them) | script message handlers in that world | `WKURLSchemeHandler` | `WKContentRuleList` (DNR) |
| SteamOS (CEF) | none in CEF 126: the page's world, in a closure | a V8 function in the renderer and process messages | the resource request handler, on the IO thread | the resource request handler (both) |
| Android | none: the page's world | a JavaScript interface | `shouldInterceptRequest` on a reserved host | `shouldInterceptRequest` (block, not redirect, for subresources) |

**Addresses.** An extension's pages and files are at an origin of its own, which
`runtime.getURL` names (`extensions::set::Addresses`). WebKit's engines give Chrome's and
Firefox's own: `chrome-extension://<id>`, `moz-extension://<id>`. Steam's Chromium holds
`chrome-extension://` for its own extension system, which lets no page reach an extension there
that it does not know itself (a content script fetching a web-accessible file, an image an
extension puts into a page), and gives `fetch` only to the schemes a program registers itself:
there Chrome's extensions are at `spatimesh-extension://<id>`, which SpatiMesh registers (as
standard, secure, CORS- and fetch-enabled, past pages' CSP), and Firefox's at
`moz-extension://<id>`. Android's WebView gives no scheme but the web's an origin: there each
extension is at `https://<id>.spatimesh-extension.invalid`, answered by the engine, never by the
network. An extension that writes `chrome-extension://` out by hand does not find its own pages
on SteamOS or Android.

Manifest V3's service worker runs as a background page (as Firefox runs its V3 background
scripts), with `importScripts` and `self` as a worker has them. Blocking `webRequest` is no part
of Chrome's Manifest V3 (only for extensions an enterprise policy installs); Firefox keeps it in
V2 and V3, and SpatiMesh offers it where the engine can stop a request. On macOS 15.4 and later
WebKit's own `WKWebExtension` could replace the bridge; the runtime above is used there too until
there is a reason not to (one behaviour everywhere).

Order: the engine-free core with tests; api.js; WebKitGTK's bridge with an extension of each
kind (a Chrome V3 one, a Firefox V2 one) running in CI; SpatiMesh's window (install, the
toolbar's buttons and badges, popups, options); CEF's bridge, in CI against CEF 126; Android's;
the Mac's.

## Media: Spatiand's own FFmpeg (decided 2026-10-16)

The owner's decision: SpatiMesh plays modern media -- H.264, H.265, AV1 (where the hardware has
it), any audio codec, MP4, WebM, MKV, and AVI, DivX and Xvid if it can -- with the FFmpeg the rest
of Spatiand uses (streaming from spatiand-host, SpatiReel), widened where it is too narrow.

* **A media file opened in SpatiMesh** (a link to one, or a page that is one) is played by
  Spatiand's media engine (`spatiand-media`: FFmpeg with VA-API, VideoToolbox or MediaCodec under
  it, software for the rest), the same engine as SpatiReel's, in the tab itself, with its
  controls: every format that engine plays, on every platform, whatever the web engine can do.
* **Video inside a page** (`<video>`, and Media Source Extensions: YouTube and the like) is the
  web engine's own pipeline, which cannot take another FFmpeg: Steam's Chromium has the codecs
  Valve built it with, WebKitGTK the distribution's GStreamer plugins (gst-libav is FFmpeg),
  WebKit on the Mac Apple's, Android's WebView the device's. A `<video>` whose file the engine
  cannot play is handed to Spatiand's engine as above.
* **Spatiand's FFmpeg**: on the Deck SteamOS's own 7.1.1 (headers vendored, libraries the
  system's: docs/remote.md), on the Mac Homebrew's, on the Beam Pro android/ffmpeg/build.sh's,
  in CI tools/ci-deps.sh's 7.1.1. The Beam Pro's is widened: dav1d for AV1 where MediaCodec has
  none, and MediaCodec wired for H.264/H.265/AV1 in the player (it decodes in software there now).

## Sound

A page's sound is made by the engine, not by us, so it is caught where it leaves the engine and
placed at the window its tab is in; a tab dragged to another window takes it along.

* **SteamOS.** CEF hands each tab's sound to the host (`CefAudioHandler`) instead of playing it.
  The host mixes the tabs of each window and plays the mix into that window's sink.
* **Other Linux.** Each tab's page plays from its own WebKit process, through PipeWire. A small
  web-process extension tells the host which process each page is in; the host moves that
  process's streams (`target.object`) to its window's sink.
* Both need **a sink per SpatiMesh window**: the compositor gives one to each window whose
  instance id asks for it (as remote applications' windows already have, keyed by id) and writes
  where they are to `$XDG_RUNTIME_DIR`. Where that fails the sound still plays, placed at
  SpatiMesh as a whole, the launch-time routing every app gets.
* **Mac.** WebKit plays every page of an engine host from one GPU process, so pages cannot be
  told apart. The host taps its own WebKit processes (a Core Audio process tap, as the Mac tap
  does for Mac apps) and hands the sound to the room (`spatiand-sound`) as the window of the tab
  that is playing (`requestMediaPlaybackState`). Two windows playing at once are heard from one.
* **Android.** WebView plays through the app's own output, with no way to take its sound, per
  tab or at all, short of screen-recording consent: **its sound is not placed in the room**.

## Settings, and what is never synced

One place, the folder every machine shares (`~/.config/spatiand/apps`, synchronised with every
linked Spatiand and spatiand-host by `spatiand-sync`, newest file winning, deletions remembered):

```text
apps/spatimesh/settings/<name>.json                one file per setting: search, custom_search,
                                                   home, zoom, developer
apps/spatimesh/profiles/<id>/profile.json          name, colour
apps/spatimesh/profiles/<id>/bookmarks/<id>.json   one file per bookmark or folder
```

A file per bookmark and per setting, so that two machines changing different ones while apart
both keep theirs; the same one changed on both: the newest wins. A setting is written only when
it changes, and the first profile, which every machine makes for itself, is written as if long
ago, so that a version of it from another machine (renamed, say) always wins over it.
`crates/spatimesh/tests/sync.rs` runs two machines through `spatiand-sync` itself: profiles,
bookmarks and settings changed apart end up the same on both, deletions included, and website
data and cache never cross. A running SpatiMesh notices within three seconds what the
synchroniser brought (the folder's newest change) and reloads it. Never in it, never synced,
thrown away freely:
website data (cookies, storage: an engine's own format, and a login is a machine's own),
`~/.local/share/spatiand/spatimesh/<profile>`; cache, `~/.cache/spatiand/spatimesh/<profile>`;
downloads in progress, in the cache until finished; history and extensions, in `local/`. The
private profile writes nothing at all.

## Built, checked, and not checked (2026-10-16)

| | Built | Checked by CI or here | Not checked: to try by hand |
|---|---|---|---|
| The window, tabs, address bar, profiles, bookmarks, settings | yes | unit tests; the window in the room's own compositor with WebKitGTK, photographed and checked pixel by pixel (`tools/spatimesh-e2e.py`: an address typed, a field typed into, a link, Back, a `<select>`) | on glasses: placement, size, reading distance; dragging a tab out into its own window and back into another (built, not exercised end to end) |
| Linux (WebKitGTK) | yes | six engine tests on Xvfb: drawing, typing, clicks, scrolling, history, new tabs, lists, dialogs, a page's process crashing alone, cookies kept or not per profile | a real desktop session |
| Mac (WKWebView) | yes | compiles (CI's macOS 15) | everything at run time: on a Mac, SpatiMesh opens pages, takes input, shows lists and dialogs; profiles keep their own cookies (macOS 14+) |
| Android (WebView) | yes | the Rust side's launch through a stand-in launcher; the Java compiles against Android 34; the wire's bytes the same from Java as from Rust; the APK builds (`spatiand-apk` artifact) | everything at run time on the Beam Pro: the launcher starts the engine's service, pages draw through the virtual display, input, a second profile in its own process, the private one leaving nothing |
| SteamOS (Steam's CEF) | yes | compiles against CEF 126's headers; CI runs the six engine tests against CEF's own 126 build laid out as Steam's | on the Deck: that Steam's CEF is still 126 (the hash check says), that its libcef.so loads outside Steam's runtime, Chromium's sandbox, Game Mode's display |
| Sync | through spatiand-sync, as Spatiand's other settings | two machines through spatiand-sync in a test (`tests/sync.rs`) | two real machines linked |
| Media through Spatiand's FFmpeg | being built (above) | -- | -- |
| Sound placed per window | **no** | -- | each engine plays its pages' sound itself, not placed at the tab's window (at most where Spatiand places SpatiMesh as a whole) |
| Extensions: the runtime (`extensions/`) | yes: manifest V2/V3, install (folder, .zip, .crx, .xpi), messages and ports, storage, tabs, windows, scripting, action, alarms, notifications, permissions, management, menus, commands, offscreen, bookmarks, webNavigation events; declarativeNetRequest's static, dynamic and session rules (`dnr.rs`: Chrome's order -- priority, then allow, block, upgrade, redirect; `urlFilter`, domains, resource types, methods, `allowAllRequests`; redirects by path, URL, substitution and transform), applied by each engine as below | unit tests against a stand-in browser; api.js run in Node for both flavours and both kinds of context (`tests/api_js.rs`) | -- |
| Extensions on WebKitGTK | yes: content scripts in each extension's own world (WebKit's patterns, a guard where they cannot say exactly), its pages' API, `chrome-extension://` and `moz-extension://` served, code run in a tab | a Chrome MV3 extension (service worker, storage, tabs.query, a web-accessible file, scripting.executeScript, a declarativeNetRequest rule blocking a fetch) and a Firefox MV2 one (background scripts, promises, its add-on id) run end to end (`spatimesh-webkitgtk/tests/extensions.rs`) | **top frame only**: content scripts in other frames run and can call, but answers and events reach only the top frame's (WebKitGTK runs outside code in no other frame; a web-process extension would); no blocking webRequest; declarativeNetRequest as WebKit's content filters: what they cannot say (`allowAllRequests` beyond the frame, redirects, header and method conditions) is left out, and `getMatchedRules` is not told |
| Extensions in the window | yes: the toolbar's buttons and badges, popups over the window, the menu's extensions (on/off, load, reload, remove), `--load-extension` | builds; not exercised in the compositor end to end | by hand: load an extension from the menu, press its button, its popup, its badge |
| Extensions on SteamOS (CEF) | yes: content scripts in the page's world, each extension's in a closure with its API kept by the engine (no world of their own in CEF 126); its pages' API as globals; Chrome's extensions at `spatimesh-extension://`, Firefox's at `moz-extension://`; code run in every frame | CI runs the shared end-to-end test against CEF 126 (the version Steam's is): the Chrome MV3 and Firefox MV2 extensions as on WebKitGTK | on the Deck, with Steam's own CEF; a page shares its world with the content scripts: it sees what they put in the DOM, and its scripts could change built-ins before theirs run; no blocking webRequest; declarativeNetRequest decided by SpatiMesh for every request (CEF asks before each one), redirects included; the end-to-end test's block rule runs in CI |
| Extensions on the Mac | yes: content scripts in a `WKContentWorld` of each extension's own, matched by WebKit's own patterns where its SPI has them (else a check in each script); its pages' API as globals; `chrome-extension://` and `moz-extension://` by a `WKURLSchemeHandler`; code run in every frame its contexts speak from | CI's Mac job (macOS 15) runs the shared end-to-end test on WKWebView, and it passes | on a Mac: everything, by hand; no blocking webRequest; declarativeNetRequest as `WKContentRuleList`s, with WebKitGTK's limits |
| Extensions on Android | yes, not yet run: each extension at `https://<id>.spatimesh-extension.invalid`, served by the engine (`shouldInterceptRequest`); a script of its own at the start of every frame's every page and a listener in every frame (the WebView's own support-library interfaces, reached by reflection as androidx.webkit reaches them), the content scripts in the page's world in a closure (as on CEF); its pages' API as globals; the top frame reached by `evaluateJavascript` through a way in that takes a secret, other frames by their listener's reply | the page script, made by the window, runs in Node as the WebView would run it (`tests/android_js.rs`); the Java compiles (CI's Android 34, and here against Android 14's own classes); the wire's bytes (both sides) | everything on the Beam Pro, first whether its WebView has document-start scripts and message listeners (the log says if not); code from the window reaches only the top frame (answers and events reach every frame); a page can hear what its other frames' contexts are told; no blocking webRequest; declarativeNetRequest not applied yet (`shouldInterceptRequest` can block and redirect only what it can fetch itself) |

## Build order

(a) this; (b) the trait and what needs no engine: tabs, address bar, profiles, bookmarks, the
settings store; (c) WebKitGTK; (d) the Mac; (e) Android; (f) SteamOS (CEF); (g) extensions;
(h) sync.
