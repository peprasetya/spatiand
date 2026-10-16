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
over the page); context menus are refused. Not done yet: each tab's sound (CEF plays it
itself for now: not placed in the room), file choosers (Chromium's own GTK one may appear),
downloads (refused). CI runs the same six tests as WebKitGTK's against CEF's 126.

More to check on the Deck: the layout of `cef_settings_t` depends on how a CEF was built
(whether it keeps the "Alloy bootstrap", as CEF's own 126 builds do); a Steam build without it
would pass the hash check and still be read wrongly. Turning on `SPATIMESH_CEF_LOG=1` makes
Chromium say what goes wrong; `SPATIMESH_CEF_OZONE=x11|wayland` picks how it talks to the
display.

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
* **SteamOS (CEF): Chrome's own extension system, with a question.** CEF's Chrome runtime is
  Chrome's code and loads unpacked Chrome extensions (`--load-extension`), Chrome's APIs and all;
  Firefox-only APIs do not exist there. Whether they act on windowless (offscreen) tabs, and how
  an extension's popup is shown without Chrome's toolbar, is to be tried against CEF 126 before
  building. CEF's older built-in subset (`CefRequestContext::LoadExtension`) covers a few APIs only
  and is gone from later CEF.
* **Other Linux (WebKitGTK): no extension API.** Only a subset we write ourselves on WebKitGTK's
  primitives, as GNOME Web does: content scripts and styles (`WebKitUserScript` and
  `WebKitUserStyleSheet` in an isolated world), messaging (`register_script_message_handler`),
  `storage`, a background page in a hidden view, the action popup, part of `tabs`, and
  `declarativeNetRequest`'s static block/allow rules as WebKit content-blocker rules. No blocking
  `webRequest`, no service-worker backgrounds, no devtools pages. Many extensions will not run.
* **Android (WebView): no extension API.** The same kind of subset on
  `addDocumentStartJavaScript`, `addWebMessageListener` and `shouldInterceptRequest`, with the
  same limits and less.

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
apps/spatimesh/settings.json                       search engine, home page, developer mode, zoom
apps/spatimesh/profiles/<id>/profile.json          name, colour
apps/spatimesh/profiles/<id>/bookmarks/<id>.json   one file per bookmark or folder
```

A file per bookmark, so that two machines adding bookmarks while apart both keep theirs; a moved
or renamed one is its own file, newest wins. Never in it, never synced, thrown away freely:
website data (cookies, storage: an engine's own format, and a login is a machine's own),
`~/.local/share/spatiand/spatimesh/<profile>`; cache, `~/.cache/spatiand/spatimesh/<profile>`;
downloads in progress, in the cache until finished; history and extensions, in `local/`. The
private profile writes nothing at all.

## Build order

(a) this; (b) the trait and what needs no engine: tabs, address bar, profiles, bookmarks, the
settings store; (c) WebKitGTK; (d) the Mac; (e) Android; (f) SteamOS (CEF); (g) extensions;
(h) sync.
