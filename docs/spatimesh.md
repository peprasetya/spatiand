# SpatiMesh, Spatiand's web browser

A full browser in the class of Safari and Chrome, written as one of Spatiand's own applications
(docs/spatiand-apps.md): a window in the room drawn with the SDK and the room's widgets, the same
program on the Mac, the Deck, other Linux and the Beam Pro. The page itself is drawn by the
platform's own web engine, behind one Rust interface. Asked for on 2026-10-16; this is the plan,
and at its end what has been built of it.

| Platform | Engine | Where it comes from |
|---|---|---|
| Mac | **WPE WebKit 2.54.1**, our own port (`third_party/wpe`, the "WPE" section below) | bundled in Spatiand.app, with GStreamer and our FFmpeg |
| Linux, the Deck | WebKitGTK (`webkit2gtk-4.1`) until WPE is built for Linux | the distribution's package |
| Android (Beam Pro) | none until WPE (wpe-android) is built | -- |

**Changed on 2026-10-25:** the first plan used each platform's own engine and bundled none.
WKWebView, Steam's CEF and the Android WebView were all built, then removed
(`spatimesh-webkit`, `spatimesh-cef`, the Android WebView service): one engine, WPE, everywhere,
with media through GStreamer on our FFmpeg. Where the sections below compare WKWebView, CEF and
the WebView, they are the record of what those engines did, kept for why WPE was chosen.

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
| A page with no window on screen | offscreen `NSWindow`, occlusion detection off (SPI `_windowOcclusionDetectionEnabled`); frames from WebKit's own layers (in this process since macOS 14) drawn by `CARenderer` on the GPU, `takeSnapshot` where they are not | CEF's windowless rendering: `OnPaint` hands each frame with what changed | `GtkOffscreenWindow`, its surface read after each draw | a `VirtualDisplay` + `Presentation` drawing into an `ImageReader` |
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

**What Steam's CEF turned out to be (on the Deck, 2026-10-17).** Steam's `libcef.so` is
Chromium 126.0.6478.183 but **Valve's own build** ("CEF 126.0.0", commit `bbb75964`, not in
CEF's repository), and its interface hash is none of CEF's. Compared with CEF 126.2.18 by its
exports and by disassembling what it does with what it is given: the same 205 functions plus ten
of Valve's (`cef_set_force_device_scale_factor`, `cef_set_minimum_font_size`, ... none
removed); `cef_settings_t` the same size (0x1c0) with every field at the same offset; but
**strings are UTF-8** (built with `CEF_STRING_TYPE_UTF8`; CEF's own builds are UTF-16), and
`cef_browser_settings_t` is 0x110 bytes, two floats of Valve's added after the last field (CEF
refuses any other size). `native/cef.c` keeps a table of Steam builds checked like this (its
hash, its string type, its browser settings' size); a build not in it is refused as before
(`SPATIMESH_CEF_ANY_BUILD=1`, with `SPATIMESH_CEF_STRINGS=utf8`, tries one anyway). With that,
on the Deck against Steam's own library, outside Steam's runtime (it loads on SteamOS itself) and
with Chromium's sandbox on: the six engine tests, the extensions test and the compositor's
end-to-end test (`tools/spatimesh-e2e.py`, all 12 checks) pass. A Steam update that changes the
hash stops SpatiMesh with a message saying so, until the new build is compared and added.

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
| Android | none: the page's world | a JavaScript interface | `shouldInterceptRequest` on a reserved host | `shouldInterceptRequest`, matched by the engine (DNR) |

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

## Media: Spatiand's own player, as part of the page (built 2026-10-17)

The owner's decision (2026-10-16, widened 2026-10-17): every film and sound in a page is played by
Spatiand's own player -- `spatiand-media`, FFmpeg with VideoToolbox, VA-API or MediaCodec under it
and software for the rest, SpatiReel's engine -- and not the web engine's; only DRM stays the
engine's. So every format plays everywhere (MKV, AVI, H.265, AV1, Opus, FLAC, AC-3...), and every
sound goes to the room at its window. It is part of the page, not laid over it: the page's scripts
see and drive the element as they would any browser's.

**In the page** (`src/media/page.js`, put into every frame of every document before the page's
own scripts, in the page's world):

* **HTMLMediaElement** is taken over: `src` and `<source>` (the engine is never given them; the
  page reads back what it wrote), `load`, `play()` and its promise (with browsers' rule for what may
  start by itself), `pause`, `currentTime`, `duration`, `paused`, `ended`, `seeking`, `readyState`,
  `networkState`, `buffered`, `seekable`, `volume`, `muted`, `playbackRate`, `loop`, `autoplay`,
  `preload`, `poster`, `controls`, `videoWidth`/`videoHeight`, `canPlayType`, `error`, and every
  media event, in browsers' order; the engine's own events for these elements are swallowed. A
  camera's stream (`srcObject`) and anything given media keys stay the engine's.
* **The picture**: the element's box is painted a key colour, `rgb(1,254,3)`; the window draws the
  film under the page where the box is (by `object-fit` and `object-position`), and draws the page
  over it with the key see-through. Where the page draws over the film half see-through (a caption
  bar, a gradient, anti-aliased text), how much of the key is left in a pixel says how see-through
  it is, and the film shows through by as much -- exact for anything that is not itself green.
  The poster is the engine's until there is a picture. Controls asked for with `controls` are
  SpatiMesh's own, drawn by the window and done through the element.
* **Web Audio**: every `AudioContext`'s destination is a node of SpatiMesh's; the mix goes to the
  window (and the engine is given silence), and from there to the room at the window.
  `OfflineAudioContext` is left alone.
* **Scripts reading the picture**: `drawImage`, WebGL's `texImage2D`/`texSubImage2D`,
  `createImageBitmap`, WebCodecs' `VideoFrame` and `captureStream` are given a canvas of the film's
  size holding its latest picture; `requestVideoFrameCallback` is called once per picture. The
  window sends pictures (read back from the GPU, JPEG, at most 1280 across and 30 a second) only
  while they are used, as wide as the page draws them, and with every seek for a film kept out of sight or asked for with
  `crossOrigin` (a thumbnail-maker); `seeked` waits for that picture.
* **MediaSource** (YouTube, most streaming players), `SourceBuffer`, `SourceBufferList`,
  `isTypeSupported`, their object URLs and Safari's `ManagedMediaSource` are SpatiMesh's: appends go
  to the window and finish, with buffered ranges, removes, `abort`, `timestampOffset`, `duration`,
  `endOfStream` and errors, as the engine's would. A page that calls
  `requestMediaKeySystemAccess` (DRM) keeps the engine's MediaSource from then on.
* **Tracks**: `textTracks` are SpatiMesh's. A `<track>`'s WebVTT or SubRip file is read in the
  page; a film's own subtitles (text, or a disc's PGS/VobSub pictures) come from the player a cue
  at a time, a little ahead of being due; cues showing are drawn by the page over the film. The
  page sees TextTrack, cues, activeCues, enter/exit/cuechange, `addTextTrack`, `<track>`'s
  `readyState` and `load`. `audioTracks` and `videoTracks` are the film's; enabling a sound track
  plays it (one at a time).
* **A MediaSource in a worker**: every Worker starts with a MediaSource of SpatiMesh's in it (its
  own script loaded after, `location`, `importScripts`, `fetch` and XHR still resolving against
  its own address); it speaks to the page, which speaks to the window as for its own, and the
  `handle` the worker hands over becomes a `MediaSourceHandle` a film's `srcObject` takes.
  `MediaSource.canConstructInDedicatedWorker` is true. A page whose policy forbids blob: workers
  keeps its worker as it was, without this.
* Frames below the top one speak to the window through their parent (postMessage), which moves
  what they say about places onto its own page; only the top frame talks to the engine.
* **Fullscreen** is SpatiMesh's on every engine (WebKit offers an embedded view none): the
  element is fixed over its frame's viewport (a frame's element over its parent's), the window
  gives the page all of itself with no bars, and Escape, another tab or a new page ends it. The
  page sees `requestFullscreen`, `fullscreenElement`, `exitFullscreen`, the `fullscreenchange`
  events and their `on…` handlers (players look for those before they trust the API), WebKit's
  prefixed names, and `screen` the size of the viewport while it lasts.

**In the window** (`src/browser/media.rs`): one sound output per tab (`spatiand_media::SoundGroup`)
that every film, `<audio>` and AudioContext of it is mixed into, each keeping its own clock; a
player per element, opened on a thread (paused until
the page says play); its time, readiness, buffering, seeks and end told to the page; what the
player cannot open (a server that wants the page's cookies) handed back to the engine. MediaSources
are `spatiand_media::mse`: each SourceBuffer's bytes read by FFmpeg's own demuxer as they come (a
new one from each initialization segment, found by MP4's and WebM's top-level boxes and elements),
coded frames kept by time per track with what an append covers taken out first (the MSE
specification's coded frame removal), an append done when its demuxer has read it all;
`Player::open_source` plays them with a reader that walks the tracks in decode order from wherever
a seek puts it, hands a new initialization segment's description to the decoder (a change of
quality), and ends with the page's `endOfStream`, which makes the length where the media ends.

**Each engine's part** is only the script and its way out (`Engine::set_page_script`,
`Happened::Media`; the window answers with `run_script` in the top frame): a message handler on
WebKit and WebKitGTK, a V8 function sending a process message on Steam's Chromium (the script
travels to the pages' process with each browser), a JavaScript interface and document-start script
on Android. `SPATIMESH_MEDIA=engine` leaves media to the engine; built without the `media` feature
(no FFmpeg), so is everything.

**Checked**: on the Mac (WebKit) and the Deck (Steam's Chromium), in the room's compositor: an MKV
of H.264 and Opus with a half see-through caption over it (VideoToolbox, VA-API), the page's events
and `play()` as a browser's, a WebM with SpatiMesh's controls, a page's Web Audio at its level
(Mac), `drawImage` of a detached film seeked for a thumbnail, a playing film drawn into a canvas
every frame and made a WebGL texture, `requestVideoFrameCallback` at the film's rate (Mac),
MediaSources of H.264 and AAC in fragments (buffered, seek, end), VP9 and Opus WebM appended in
uneven slices and a change of quality halfway (Mac); YouTube streaming through it (Mac, AV1), its
fullscreen ('f', Escape) with the film playing throughout; three `<audio>`, a film and Web Audio on
one page through one output, each keeping time (Mac); a `<track>` shown, an MKV's SubRip track and
second sound track chosen by the page, a worker's MediaSource played (Mac).
**Not yet**: anything on the Beam Pro; WebKitGTK only by CI's engine tests; in the glasses; sound
heard (tests run muted); `<track>` cue settings (position, line, alignment: cues are always at
the bottom, centred); a disc's picture subtitles in a page (the path is there, untested); AV1 offered to pages where the machine can only decode it in software.

**Load (2026-10-17, 2026-10-18).** Pages are offered only codecs this machine decodes in hardware
(`spatiand_media::hardware`: VideoToolbox's answer on a Mac; everything elsewhere, for now), and
`SPATIMESH_SOFTWARE_CODECS` overrides the list. **On a Mac VP9 and AV1 are never offered**: they
are supplemental decoders, run in a service of their own, and VP9's makes one picture of a
MediaSource's pieces and then never answers (`VTDecompressionSessionWaitForAsynchronousFrames`;
sampled 2026-10-18), and a second film opened meanwhile hung SpatiMesh with it. H.264, the GPU's
own decoder, takes YouTube's pieces without fault, so YouTube streams H.264 here. A hardware
decoder that has made nothing for 1.5 s with packets waiting is still replaced by software from a
key frame, and a player that ends waits at most a second for its threads (one stuck in a decoder
is left behind). A window closed stops its pages' films; the room drops sound whose window has
gone rather than play it unplaced.

WebKit's pages reach the window **on the GPU, uncopied**: the engine host renders WebKit's own
compositing layers with Core Animation into two IOSurfaces per tab and names the one just drawn
(`FromHost::Surface`); the window samples it through ANGLE and gives the one it replaces back
(`ToHost::ReleaseSurface`). Before, `takeSnapshot` repainted pages on the CPU (YouTube's blurred
glow: 94 ms a picture), and each picture was read back, compared and copied three times more.
Pictures come at 60 a second while the page changes, 15 while a film plays, 2 when idle.

One YouTube film at 720p60 in the test room, release builds, every process of SpatiMesh's
counted (window, engine host, WebKit's page/GPU/network processes, the decoder service): about
56% of a core (2026-10-18), from 250% (2026-10-17). Safari on the same film, its window on the
Mac's screen: about 42% (the Mac's own compositing of Safari not counted, nor the room's of
SpatiMesh).

**Spatiand's FFmpeg**: on the Deck SteamOS's own 7.1.1 (headers vendored, libraries the system's:
docs/remote.md), on the Mac Homebrew's, on the Beam Pro android/ffmpeg/build.sh's, in CI
tools/ci-deps.sh's 7.1.1.

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
* **Mac (built 2026-10-17).** WebKit plays every page of an engine host from its GPU process,
  which is launchd's child and "responsible" to whatever started the session, so neither the
  room nor the Mac's own application taps can tell it is SpatiMesh's. The engine host asks the
  web view for its GPU and page processes (`_gpuProcessIdentifier`, `_webProcessIdentifier`,
  SPI) when a tab plays, taps them itself (`spatimesh-webkit/native/sound.m`: a Core Audio
  process tap, the original muted only while it is read, under the Spatiand app's permission to
  record what the Mac plays) and writes it to the session's `spatiand-sound` socket under the
  window process's `SPATIAND_APP_ID`, as SpatiReel does. In the room it is placed at SpatiMesh's
  window through the HRTF; in 2D mode it plays unplaced, as plain stereo; with no session it is
  not tapped. One stream for the whole engine host: a window whose tab is playing says so in its
  title (a speaker first), which is the window the room places it at. Checked: a page's tone
  arrives at the session at the level it was played (`--selftest-desk SpatiMesh <folder>
  <address>` run as the app, `SPATIAND_DEBUG_OWN_SOUND=1`); not yet heard in the glasses.
* **Android.** WebView plays through the app's own output, with no way to take its sound, per
  tab or at all, short of screen-recording consent: **its sound is not placed in the room**.
* **Each window its own place (Mac and Beam Pro, 2026-10-17).** Everything SpatiMesh plays itself
  (its media, above) goes out per tab, under the name of the tab's window: `spatiand_xr_v1`
  version 8's `set_sound_key` tells the room which window a name belongs to. The first window
  is named as the process is, which the engine's own tap uses too; the room mixes every stream
  under one name before the HRTF, so the two never take turns in one slot.

## Settings, and what is never synced

One place, the folder every machine shares (`~/.config/spatiand/apps`, synchronised with every
linked Spatiand and spatiand-host by `spatiand-sync`, newest file winning, deletions remembered):

```text
apps/spatimesh/settings/<name>.json                one file per setting: search, custom_search,
                                                   home, zoom, developer, restore, bookmarks_bar
apps/spatimesh/profiles/<id>/profile.json          name, colour, place in the list, lock (a PIN's
                                                   or password's salted hash, 100 000 rounds of
                                                   SHA-256)
apps/spatimesh/profiles/<id>/settings/<name>.json  the profile's own: search, custom_search
apps/spatimesh/profiles/<id>/bookmarks/<id>.json   one file per bookmark or folder
```

A file per bookmark and per setting, so that two machines changing different ones while apart
both keep theirs; the same one changed on both: the newest wins. A setting is written only when
it changes, and the first profile, which every machine makes for itself, is written as if long
ago, so that a version of it from another machine (renamed, say) always wins over it.
`crates/spatimesh/tests/sync.rs` runs two machines through `spatiand-sync` itself: profiles,
bookmarks and settings changed apart end up the same on both, deletions included, and website
data and cache never cross. A running SpatiMesh notices within three seconds what the
synchroniser brought (the folder's newest change) and reloads it. Cookies and history are in it
too (below). Never in it, never synced, thrown away freely: the engine's own website data
(storage, its cookie database), `~/.local/share/spatiand/spatimesh/<profile>`; cache,
`~/.cache/spatiand/spatimesh/<profile>`; downloads in progress, in the cache until finished;
extensions, in `local/`; what each profile had open (`local/spatimesh/profiles/<id>/session.json`)
and the list of downloads (`local/spatimesh/downloads.json`). The private profile writes nothing
at all.

## Logins and history on every machine (2026-10-19)

Log in on the Mac, be logged in on the Beam Pro and the Deck; a page visited on one is a visited
link on the others.

```text
apps/spatimesh/profiles/<id>/cookies/<site>.json              a site's cookies (its domain, no dot)
apps/spatimesh/profiles/<id>/history/<machine>-<YYYY-MM>.json  a machine's visits in a month
local/spatimesh/profiles/<id>/cookies.json                    what engine and files last agreed on
local/spatimesh/machine.json                                  this machine's name for its files
```

**Cookies** (`crates/spatimesh/src/cookies.rs`, the window's side in `browser/kept.rs`). Each
engine keeps cookies in its own format, so the window asks its engine for all of them
(`ToHost::AskCookies`, answered by `Happened::Cookies`) three seconds after a page has loaded,
every minute, and when the synchroniser brought something; it merges three ways, site by site,
against the files and what the two last agreed on: changed here is written out, changed
elsewhere is given to the engine (`ToHost::SetCookies`), removed on either side (a logout) is
removed on both. Only cookies that outlive the session cross (an engine forgetting its session
cookies when it ends must not log the other machines out), and an expiry moved by less than a day
is not a change (sites renewing theirs on every page would write a file each time). An answer to
an ask made before cookies were given to the engine is thrown away and asked again. The engines:
WKWebView's cookie store; WebKitGTK's cookie manager (2.42 and later); Steam's Chromium's global
cookie manager; Android's WebView cannot list its cookies, so it is asked what it would send to
each address the window names (each known cookie's own, and the hosts of recent visits), and the
window makes the cookies out of that (`Happened::CookiesSent`): a known cookie keeps what is known
of it, a new one is taken as the host's own, kept a week. The cookies are in the shared folder as
the engines keep theirs on disk, readable by whoever can read the folder; a profile's lock guards
its windows, not these files.

**History** (`crates/spatimesh/src/history.rs`). A page is remembered when it has loaded (its
title brought up to date when it comes after); each machine writes only its own month's file, so
the synchroniser never has two versions of one; a machine removes its own files over a year old.
Every engine is told every address visited on any machine (`ToHost::Visited`): WKWebView's
visited-link store (`_WKVisitedLinkStore`, SPI; a bare WKWebView shows no link as visited, so it
is told this machine's visits too) and Android's `WebChromeClient.getVisitedHistory` show them as
visited; WebKitGTK and Chromium have no way to be told, and show only their own.

Checked: unit tests (two machines' folders: a login reaching the other, a logout, both changed
while apart, session cookies and renewals, Android's answers made into cookies; visits);
`tools/spatimesh-sync-e2e.py`, two SpatiMesh homes sharing one folder in the room's snapshot
compositor -- on this Mac (2026-10-19): A's HttpOnly login written out, B's request carrying it,
B showing A's page as visited; in CI with WebKitGTK; each engine's cookies said, made and removed
(`cookies_are_said_made_and_removed`, WebKitGTK and CEF, in CI). Not yet run: Android (no device
here), the Deck's Steam Chromium itself, real sites' logins (Google may bind a login to the
device it was made on).

## Profiles: whose window this is, and its lock (2026-10-18)

Beside the menu button, a pill of the profile's colour and name; the window's bars are washed in
that colour and its page framed in it. The pill's list: the profiles (choosing another opens a
window of it), a new profile, **Settings of** the window's profile, and **Lock profiles now**.
The settings: its name, what its windows ask for (nothing, a PIN of 4 to 12 digits, or a password
of at least 6 characters; typed twice; changed or removed only after the present one), and its
colour.

A profile with a lock asks for it **when a window of it opens and none of its windows is open
here**; once typed, more windows of it open without asking, until the last of its windows closes
or the padlock beside the pill (or Lock profiles now) locks every profile that has one. A locked
window shows only the profile's colour, its name and the field: not its tabs, not its address,
and its title says only that it is locked; its pages' sound is hushed (they play on, unheard).
Five wrong tries in a row and the next waits half a minute. The lock keeps a window from being
looked at; it does not encrypt the profile's website data on disk.

## The browser around the page (2026-10-27)

What a browser has around its pages, all of it SpatiMesh's own (the window process, drawn with
the room's widgets): nothing here is the engine's but the few hooks listed last, so it is the same
on the Mac, Linux and the Beam Pro.

* **Menus.** The menu button's list has submenus, opened beside the line that names them (by the
  pointer over it, or Right): History (closed lately, visited lately), Bookmarks, Zoom, Search
  engine, Extensions. Twelve lines at most: a room's line is tall.
* **A right click** shows what can be done with what is under it, as the engine says
  (`Happened::ContextMenu`, from its own hit test, only when the page did not take the click): a
  link (open in a new tab, window or private window, copy its address or words, save it), an
  image (open, save, copy its address), a film or sound (play or pause, mute, loop, its controls,
  open, save, copy -- its state asked of the page when the menu opens), chosen text (copy, search
  for it, go to it if it is an address), a field (undo, redo, cut, copy, paste, choose all), the
  page itself (back, forward, reload, bookmark, save, find, choose all), and Inspect. On a tab:
  new tab after it, reload, duplicate, pin, mute, to a new window, close, close the others or
  those after it, reopen closed. On a bookmark, a line of SpatiMesh's own pages, a most visited
  site: their own.
* **The address bar** suggests, as it is typed into, pages of the history (on every machine),
  the bookmarks and the open tabs (`crate::omnibox`: every word typed must be in a page; the
  start of its address counts most, then the start of a word of its title or of a part of its
  address; often and lately visited first), and finishes the likeliest address inline (to the
  site while the typing is within its name), the rest chosen so that typing on replaces it. Up
  and Down go through them, Enter takes one, Shift-Delete takes a page out of the history. A
  padlock before it over HTTPS, a warning over plain HTTP; pressed, what that means.
* **Search engine per profile**, chosen in its submenu or the settings page, or an address of the
  profile's own; kept with the profile (`profiles/<id>/settings/`), so on every machine.
* **Profiles in an order of their own**: a profile's settings move it up or down the list (each
  profile is given its place, `profile.json`'s `position`), and remove it (its name typed again).
* **Tabs**: pinned tabs at the start of the bar, small; a sounding tab's speaker, pressed to
  silence it; Control-Shift-T opens the last closed tab or window again, History's submenu the
  others (25 kept, this run only). Control-PageUp and PageDown go between tabs, with Shift move
  the tab along.
* **What was open, opened again.** Each profile's windows are written down while they change
  (every ten seconds at most) and when its last window closes; a profile opened with none of its
  windows open -- at start, from the profile button, another start of SpatiMesh -- opens them as
  they were, each tab on its bar but only the one in front loaded (the others when first shown).
  Not for the private profile. Settings, "When a profile opens", can make it its home page instead.
* **Bookmarks**: the star bookmarks the page and opens its bookmark's menu (rename, remove, all
  bookmarks); the bookmarks bar under the toolbar (Control-Shift-B); the Bookmarks page.
* **Find in the page** (Control-F, F3, Control-G): a bar over the page's top right; the engine
  marks every match (`Engine::find`), and counts them.
* **Downloads**: a link's, an image's, a film's, the page's own ("Save page as", Control-S), and
  what a page downloads itself; into `~/Downloads` under a free name. A button on the toolbar
  while there are any, with their progress; the Downloads page (Control-J) opens, shows, stops,
  tries again. A downloaded extension package (`.crx`, `.xpi`) is offered for installing.
* **Extensions from their stores.** The Chrome Web Store and Firefox Add-ons offer a browser that
  is neither no button: on a store's page for one extension the toolbar has "Add to SpatiMesh",
  which downloads its package (`crate::extensions::store`: Chrome's from Google's update service,
  Firefox's as its latest version) into the profile's cache and installs it as a file would be.
  The Extensions page (and submenu) link to both stores, load an unpacked folder or a package
  from the files, and turn each extension on or off, open its options, remove it.
* **Choosing files.** The room has no file chooser, so SpatiMesh has one: places (home,
  Downloads, Documents, Desktop, Pictures, the whole machine), the folder's own, a field to find
  in it or to type a path into; for loading an extension, and for a page's `<input type=file>`
  (`Happened::ChooseFiles`, answered with `Engine::choose_files`), one file or several, of the
  kinds the page asked for (or every kind, asked).
* **SpatiMesh's own pages**, at `spatimesh:` addresses the engine is never given (it shows a blank
  page under them): History (searched, by day, a page's menu, Delete to take it out, clearing the
  last hour, today or all of it), Bookmarks, Downloads, Settings, Extensions. Lines that take
  something away are at the end, where Enter on the first line cannot reach them by chance.
  Taking a page out of the history reaches every machine: each machine keeps a list of what it
  was told to forget (`history/forgotten-<machine>.json`), which every machine leaves out of what
  it shows, and rewrites its own files without it.
* **The start page**: the sites visited most (a tile each), and the bookmarks.
* **The link under the pointer** at the foot of the page (`Happened::Hovered`).
* Keys: Control-T, -W, -N, -Shift-N, -Shift-T, -L (-E, -K, F6), -R (F5), -D, -F, -G (F3), -H, -J,
  -Shift-O, -Shift-B, -S, -+, --, -0, -1 to -9, -Tab, -PageUp, -PageDown; Alt-Left, -Right, -Home;
  Control-Shift-I, Control-Alt-I and F12 for the developer tools.

**The engines' part** (`Engine` and the wire, in both C engines, `spatimesh-webkitgtk` and
`spatimesh-wpe`, on WebKit's public API): the context menu's hit test (`context-menu`), the link
under the pointer (`mouse-target-changed`), `WebKitFindController`, the file chooser request, and
downloads (`webkit_web_view_download_uri`, every download the network session starts, a free name
in Downloads; WebKitGTK 4.1's destination is a URI, WPE's a path). WebKit itself is not changed.
The flat-colour engine answers a right click and finding too, so the window can be tried without
one.

**Checked**: unit tests (suggestions and inline finishing, history entries and forgetting across
two machines, sessions, profile order, a profile's search engine, store pages, downloads' sizes,
pinned tabs, the wire); the WebKitGTK engine on Xvfb (`menus_links_under_the_pointer_and_finding`,
`downloads_and_files_for_a_page`: a menu over a link and a field, the link under the pointer,
three matches found and none, a file downloaded whole into the file named, a page's file field
answered); in the room's compositor with WebKitGTK, photographed (`tools/spatimesh-ui-e2e.py`:
every menu and page above, a link saved into Downloads whole, SpatiMesh killed and started again
with its tabs back); `spatimesh` built for Android (`cargo check --target aarch64-linux-android
--no-default-features`). **Not checked here**: the WPE engine's half (no WPE build on Linux yet:
written to WPE 2.54.1's API, checked against its source, to be built on the Mac); installing from
the stores themselves (their addresses are tested, not fetched); in the glasses; the Beam Pro.

## Visited links (found 2026-10-27)

A link followed in one tab, or opened in a new one, did not look visited in another tab of the
same profile; going back in the tab that followed it, it did. The cause is WebKit's GLib API, the
same in WebKitGTK and WPE (2.54.1): `webkitWebViewCreatePageConfiguration`
(`UIProcess/API/glib/WebKitWebView.cpp`) never gives a page configuration a visited-link store,
so every web view makes its own -- shared only with the views its pages open with
`window.open`. Not the processes: two views in one web process do not share them either. Shown by
`spatimesh-webkitgtk`'s `a_link_visited_in_another_tab_looks_visited` (ignored until fixed): a
fresh tab showing the same page shows the link unvisited. Nothing in the public API shares one:
`related-view` does (a view related to one view of the engine's), but puts every tab in that
view's web process, and one tab's crash then takes the others; tried and taken out.

The fix is WebKit's, a patch for `third_party/wpe/patches/webkit/`: one `VisitedLinkStore` per
`WebKitWebContext` (or network session), set on every page configuration it makes -- and, with
it, a call to add visited links, which would also make `Engine::add_visited` (pages visited on
other machines, and before SpatiMesh last started) work on WPE and WebKitGTK, as it did on
WKWebView. Not made yet: SpatiMesh's WebKit is not to be changed for now.

## WPE WebKit, SpatiMesh's own engine (from 2026-10-22)

The platform engines (WKWebView, Android's WebView, Steam's Chromium) each draw into their own
system and are reached around it. WPE WebKit draws into what it is given, so it is to be
SpatiMesh's engine everywhere, carried by Spatiand at one version (`third_party/wpe`), with
media through Spatiand's own FFmpeg. The Mac is first; WPE does not build on macOS upstream,
so it is ported here.

**Where it is.** `third_party/wpe/build.sh mac` builds WebKit at the `wpewebkit-2.54.1` tag with
our patch series (`patches/webkit/`) and every library it needs from source (no Homebrew), into
`~/Developer/spatiand-wpe` (outside iCloud). `crates/spatimesh-wpe` is the engine program: the
shim's C interface on WPE's 2.0 API, and SpatiMesh's own WPE platform (`MeshDisplay`, `MeshView`,
`MeshToplevel`). `mac/make-app.sh` puts it all in the app (`Contents/Frameworks/wpe`, laid out
as the build's prefix; the developer tools' pages in `Contents/Resources/wpe`); WebKit finds its
processes from where its library was loaded. It is SpatiMesh's only engine on a Mac (WKWebView,
the earlier one, is no longer built into the app; its crate stays for now).

| | State |
|---|---|
| Pictures | On the GPU, nothing copied: each frame an IOSurface ANGLE (on Metal) drew into, handed to the window and given back when it lets go (`ToHost::ReleaseSurface`). Not global surfaces any process could open by number: handed over in private, Mach port by Mach port, from the web process to the engine and from the engine to the window (`crates/spatiand-surfaces`, as every Spatiand application's pictures reach the room). |
| Media | WPE's own GStreamer player (1.28.8, built with the plugins pages need): video, Media Source Extensions (YouTube), WebAudio. Decoding by gst-libav on Spatiand's FFmpeg 9.0.2, VideoToolbox in hardware, no software AV1. SpatiMesh's own player script is not put into pages on WPE. Not yet: video frames on the GPU (they are copied into a texture now). |
| WebGL | WebKit's own ANGLE, its EGL back end running on the ANGLE the rest of WPE draws with, as on Linux it runs on Mesa (the shape upstream WPE has). Not yet: WebKit's ANGLE on Metal handing its pictures over as IOSurfaces, which would skip a layer. |
| Fonts | fontconfig with Spatiand's own `fonts.conf`: each script's macOS font by language; WOFF2 web fonts (Google's woff2, on brotli); the interface font reported by SpatiMesh's display (WPE had none for form controls: patched). |
| Developer tools | WebKit's Web Inspector, as a tab after its page (`ToHost::Inspect`; the menu's "Developer tools", Ctrl-Alt-I or Command-Option-I). It closes with its page. Not on WebKitGTK or WKWebView, whose inspector is a window of its own. |
| Files | `file://` pages load (GIO's type identifiers taken as MIME types, patched). |
| Extensions | The shared end-to-end test passes. |
| Not yet | Video frames as IOSurfaces into ANGLE; WebGL on WebKit's ANGLE on Metal; Linux and Android builds of WPE; the developer tools' text size in the glasses; the interface font from each platform's own. |

## Built, checked, and not checked (2026-10-16)

| | Built | Checked by CI or here | Not checked: to try by hand |
|---|---|---|---|
| The window, tabs, address bar, profiles, bookmarks, settings | yes | unit tests; the window in the room's own compositor with WebKitGTK, photographed and checked pixel by pixel (`tools/spatimesh-e2e.py`: an address typed, a field typed into, a link, Back, a `<select>`) | on glasses: placement, size, reading distance; dragging a tab out into its own window and back into another (built, not exercised end to end) |
| Linux (WebKitGTK) | yes | six engine tests on Xvfb: drawing, typing, clicks, scrolling, history, new tabs, lists, dialogs, a page's process crashing alone, cookies kept or not per profile | a real desktop session |
| Mac (WKWebView) | yes | compiles (CI's macOS 15); the extensions test (CI and here); **the compositor end to end on this Mac** (`tools/spatimesh-e2e.py` with the Mac's snapshot compositor and `SPATIMESH_ENGINE=webkit`: all 12 checks, 2026-10-17) | on glasses; profiles keeping their own cookies (macOS 14+) |
| Android (WebView) | yes | the Rust side's launch through a stand-in launcher; the Java compiles against Android 34; the wire's bytes the same from Java as from Rust; the APK builds (`spatiand-apk` artifact) | everything at run time on the Beam Pro: the launcher starts the engine's service, pages draw through the virtual display, input, a second profile in its own process, the private one leaving nothing |
| SteamOS (Steam's CEF) | yes | compiles against CEF 126's headers; CI runs the six engine tests against CEF's own 126 build laid out as Steam's; **on the Deck against Steam's own CEF** (Valve's build, above): the six engine tests, the extensions test and the compositor end to end, sandbox on, outside Steam's runtime (2026-10-17) | Game Mode's display; on glasses |
| Sync | through spatiand-sync, as Spatiand's other settings | two machines through spatiand-sync in a test (`tests/sync.rs`) | two real machines linked |
| Media through Spatiand's own player, part of the page | yes: files, Web Audio, pictures for scripts, MediaSource (above) | Mac and Deck in the compositor, test pages and YouTube (above) | the Beam Pro; in the glasses; by ear |
| Sound placed per window | Mac: the engine host's sound to the room, at the window that plays (above); Deck: SpatiMesh's sink, which Steam's Chromium inherits | Mac: a page's sound reaches the session at its level | in the glasses, by ear; the Deck's by ear; two SpatiMesh windows playing at once are one stream |
| Extensions: the runtime (`extensions/`) | yes: manifest V2/V3, install (folder, .zip, .crx, .xpi), messages and ports, storage, tabs, windows, scripting, action, alarms, notifications, permissions, management, menus, commands, offscreen, bookmarks, webNavigation events; declarativeNetRequest's static, dynamic and session rules (`dnr.rs`: Chrome's order -- priority, then allow, block, upgrade, redirect; `urlFilter`, domains, resource types, methods, `allowAllRequests`; redirects by path, URL, substitution and transform), applied by each engine as below | unit tests against a stand-in browser; api.js run in Node for both flavours and both kinds of context (`tests/api_js.rs`) | -- |
| Extensions on WebKitGTK | yes: content scripts in each extension's own world (WebKit's patterns, a guard where they cannot say exactly), its pages' API, `chrome-extension://` and `moz-extension://` served, code run in a tab | a Chrome MV3 extension (service worker, storage, tabs.query, a web-accessible file, scripting.executeScript, a declarativeNetRequest rule blocking a fetch) and a Firefox MV2 one (background scripts, promises, its add-on id) run end to end (`spatimesh-webkitgtk/tests/extensions.rs`) | **top frame only**: content scripts in other frames run and can call, but answers and events reach only the top frame's (WebKitGTK runs outside code in no other frame; a web-process extension would); no blocking webRequest; declarativeNetRequest as WebKit's content filters: what they cannot say (`allowAllRequests` beyond the frame, redirects, header and method conditions) is left out, and `getMatchedRules` is not told |
| Extensions in the window | yes: the toolbar's buttons and badges, popups over the window, the menu's extensions (on/off, load, reload, remove), `--load-extension` | builds; not exercised in the compositor end to end | by hand: load an extension from the menu, press its button, its popup, its badge |
| Extensions on SteamOS (CEF) | yes: content scripts in the page's world, each extension's in a closure with its API kept by the engine (no world of their own in CEF 126); its pages' API as globals; Chrome's extensions at `spatimesh-extension://`, Firefox's at `moz-extension://`; code run in every frame | CI runs the shared end-to-end test against CEF 126 (the version Steam's is): the Chrome MV3 and Firefox MV2 extensions as on WebKitGTK | on the Deck, with Steam's own CEF; a page shares its world with the content scripts: it sees what they put in the DOM, and its scripts could change built-ins before theirs run; no blocking webRequest; declarativeNetRequest decided by SpatiMesh for every request (CEF asks before each one), redirects included; the end-to-end test's block rule runs in CI |
| Extensions on the Mac | yes: content scripts in a `WKContentWorld` of each extension's own, matched by WebKit's own patterns where its SPI has them (else a check in each script); its pages' API as globals; `chrome-extension://` and `moz-extension://` by a `WKURLSchemeHandler`; code run in every frame its contexts speak from | CI's Mac job (macOS 15) runs the shared end-to-end test on WKWebView, and it passes | on a Mac: everything, by hand; no blocking webRequest; declarativeNetRequest as `WKContentRuleList`s, with WebKitGTK's limits |
| Extensions on Android | yes, not yet run: each extension at `https://<id>.spatimesh-extension.invalid`, served by the engine (`shouldInterceptRequest`); a script of its own at the start of every frame's every page and a listener in every frame (the WebView's own support-library interfaces, reached by reflection as androidx.webkit reaches them), the content scripts in the page's world in a closure (as on CEF); its pages' API as globals; the top frame reached by `evaluateJavascript` through a way in that takes a secret, other frames by their listener's reply | the page script, made by the window, runs in Node as the WebView would run it (`tests/android_js.rs`); the Java compiles (CI's Android 34, and here against Android 14's own classes); the wire's bytes (both sides) | everything on the Beam Pro, first whether its WebView has document-start scripts and message listeners (the log says if not); code from the window reaches only the top frame (answers and events reach every frame); a page can hear what its other frames' contexts are told; no blocking webRequest; declarativeNetRequest matched by the engine itself (`Dnr.java`, on rules the window digests; its own test runs in CI): the WebView says only whether a request is the top frame's, so the rest's resource type is guessed from its Accept header and its address's ending; a blocked request is answered with a body that cannot be read (that this fails it as a network error does is untested); a subresource's redirect is answered with the extension's file, or with what a GET made by the engine gets (with the page's cookies; not other methods); the top frame's is a new load |

## Build order

(a) this; (b) the trait and what needs no engine: tabs, address bar, profiles, bookmarks, the
settings store; (c) WebKitGTK; (d) the Mac; (e) Android; (f) SteamOS (CEF); (g) extensions;
(h) sync.
