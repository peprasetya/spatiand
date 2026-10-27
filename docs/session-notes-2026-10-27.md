# Session notes, 2026-10-27: what was built and changed, and what to do on the Mac

One cloud session's work, merged into `master` as 13 commits (from `a54bc88` to the commit
that adds this file). The design and status live where they always do -- `docs/spatimesh.md` and
`docs/spatiand-apps.md` -- and these notes are the map: what changed where, what was checked
here, what was not, and what to do next on the Mac.

## Picking it up on the Mac

```sh
git pull                                   # master
third_party/wpe/build.sh mac               # rebuilds WPE with the new patches (below)
cargo build --release -p spatimesh -p spatimesh-wpe -p spatishelf -p spatiquill
mac/make-app.sh                            # as before
```

**The WebKit patches apply themselves.** `build.sh` now applies to an existing checkout every
patch in `third_party/wpe/patches/webkit/` it has not had yet, and writes down which it has in
`$WPE_WORK/webkit-2.54.1/.spatiand-patches`. A checkout from before that list (the Mac's, with
only 0001) is worked out from the last patch that comes off it in reverse: 0001. So on the Mac
it applies 0002 to 0006 and builds. If a patch does not apply, `git apply` stops the script and
says which.

**What is most likely to need a hand on the first Mac build:** patch 0004 (WebGL on Metal), which
is Mac-only code written without a Mac to compile it -- see below. Without it everything else
still builds: set it aside by moving the file out of `patches/webkit/` before the first run.

## WebKit patches (third_party/wpe/patches/webkit)

| Patch | What | Checked |
|---|---|---|
| 0002-visited-links-per-context | One `VisitedLinkStore` per `WebKitWebContext`, so a profile's tabs share visited links; `webkit_web_context_add_visited_links` / `_clear_visited_links`, announced by `WEBKIT_WEB_CONTEXT_HAS_VISITED_LINKS`. | Linux: both visited-link tests pass (below). |
| 0003-relocatable-on-linux-and-android | `RelocatedInstallPath.h` for Linux and Android too (paths found from where the library was loaded, `dladdr`); on Android WebKit's processes are found as `lib<name>.so` beside the library, and the injected bundle likewise. | Linux: built (the relocation itself is used by the bundle, not by `SYSTEM_DEPS`). Android: not built. |
| 0004-mac-webgl-on-metal-onto-iosurfaces | WebGL on WebKit's ANGLE's own Metal back end, drawing onto three IOSurfaces in turn, which the compositor's ANGLE takes as textures of the same memory, with a fence the compositor waits on. New: `GraphicsContextGLTextureMapperIOSurface.{h,cpp}` and `...IOSurfaceImport.cpp` (libepoxy's half); ANGLE's Metal sources added to `PlatformWPE.cmake`. `WEBKIT_WEBGL_DISABLE_IOSURFACE=1` goes back to the old way; so does anything missing (it falls back by itself). | Linux: applies and builds (the code is `#if OS(DARWIN)`, so compiles to nothing there). **Mac: never compiled.** |
| 0005-preconnect-api | `webkit_web_view_preconnect`: a connection (DNS, TCP, TLS) made before a load, through WebKit's own `WebPageProxy::preconnectTo`; announced by `WEBKIT_WEB_VIEW_HAS_PRECONNECT`. | Linux: builds; the engines call it. Its effect (time saved) was not measured. |
| 0006-media-page-out-of-line | 0001's `HTMLMediaElement::mediaPlayerSpatiandPage` moved out of the header: Linux's build of WebKit's UIProcess could not compile it there (`Document` incomplete). | Linux: built. |

**WebGL on the Mac, before 0004** (the question asked): every frame of a page already reaches the
window as an IOSurface with nothing copied, WebGL's canvas in it; what WebGL draws is a texture of
the compositor's ANGLE, shared, also not copied. What 0004 removes is the second ANGLE in between
(WebKit's ANGLE on its GL back end over the system's): every call and shader translated twice.

What to check on the Mac once 0004 builds: `webgl_draws` in `crates/spatimesh-wpe/tests/engine.rs`;
a WebGL page (three.js examples, a WebGL game) in SpatiMesh; the log for "Failed to create a
graphics context for WebGL on Metal" (it fell back) ; and then with
`WEBKIT_WEBGL_DISABLE_IOSURFACE=1` to compare.

## third_party/wpe/build.sh

* **`linux`** -- the bundle for SteamOS, to be built in the Ubuntu 22.04 root: every library but
  glibc, libstdc++, Mesa's EGL/GLES, OpenSSL 3, zlib, bzip2, expat, libffi, libva and PulseAudio's
  client, made to find each other by `$ORIGIN` (patchelf). The packages the 22.04 root needs are
  in the script's header. New pins in `versions.sh`: ICU 76.1, libxml2 2.13.8, SQLite 3.50.4.
  **Not run here**: this machine is 24.04 and could not reach download.gnome.org, gnupg.org,
  gstreamer.freedesktop.org, sourceforge, savannah, sqlite.org or xkbcommon.org.
* **`SYSTEM_DEPS=1 ... linux`** -- the same on the distro's own libraries, for working on it.
  **Built here** (Ubuntu 24.04, 4 cores, about 6 hours) with all six patches.
* **`android`** -- an NDK cross build (arm64, API 34): CMake, meson and autotools told of the
  target; ICU cross-built from a build of it for the building machine; libffi, expat, libintl
  built (Bionic has not got them); every library renamed to an unversioned `lib*.so` and WebKit's
  processes put beside them as `lib<process>.so`. New pins: libffi 3.4.8, expat 2.7.1. **Not run**
  (no NDK here). The step most likely to fail first: libgpg-error, which has no lock description
  for Android; the script copies glibc's arm64 one, which may need replacing with one generated on
  a device (`gen-lock-obj.sh`).
* While a build runs, `build.sh` can be edited if the build was started from a copy
  (`.build-run.sh`, ignored by git).

## SpatiMesh

* **Visited links** across tabs and machines: the window tells the engine what the history has
  (`ToHost::Visited`) and, when pages are taken out of it (here or on another machine), has it
  forget them all and tells it again what is left (`ToHost::ForgetVisited`, new). On the distro's
  WebKitGTK (no 0002) links stay per tab, as before.
* **Preconnect** (`ToHost::Preconnect`, new): on a link pointed at, and to where the address bar
  would go as it is typed; once in 30 seconds a server. DNS prefetch only where the engine has not
  got 0005 (WebKitGTK).
* **Networking**, as asked: it was already as parallel as a browser's (a network process per
  profile, 256 connections, 6 to a server, HTTP/2, TLS on threads, the disk cache on its own); the
  change is not waiting (preconnect), not more threads. See `docs/spatimesh.md`, "Networking".
* **Drops**: files and links dropped on the tabs open in a new tab; on the page, there.
* **Engine choice**: on Linux and Android, `spatimesh-wpe` when Spatiand's WPE is beside it
  (`wpe/lib` on Linux, `libWPEWebProcess.so` on Android), else WebKitGTK (Linux) or none (Android).
* `spatimesh-wpe`: finds the Linux bundle in `wpe/` beside it (and fontconfig the system's
  `/etc/fonts`); on Android, links to GIO's and GStreamer's modules in the app's cache, Android's
  fonts (`third_party/wpe/android/fonts.conf`), the registry made in-process; reads Android's
  AHardwareBuffer pictures back (CPU, else GL); where the build's GStreamer folder is only
  Spatiand's sink, adds it to the system's plugins instead of replacing them.
* Fixed: a start-page tile's colour no longer depends on the port (the UI e2e check flaked).
* Release: `tools/make-release.sh` carries `spatimesh`, `spatimesh-wpe` and `wpe/` when the bundle
  has been built; `tools/install-release.sh` installs them.

## SpatiShelf (crates/spatishelf; new `src/hand.rs`)

Drag and drop (onto folders, the side's places, tabs -- which come forward when held over -- and
the path; move on a disk, copy across, Alt copies, Shift moves); a band to choose several; rename
in place as in Finder (click the chosen name, F2, or the menu; the ending not chosen); the right
button's menu for a thing or for the folder, opaque, with icons and keys (Move to..., Copy to...,
Duplicate, Paste into, Open in a new tab, Sort by, SpatiQuill here); Control-click on a Mac, the
menu key, Shift-F10 and a press held still open it too; sorting by name (numbers by value), kind,
size, date, folders first, the list's heads clickable; Ctrl/Cmd-D duplicates.

## Drag and drop between windows, in the room

* **spatiand-app**: `App::start_drag` (forms and bytes, a picture), `accept_drag`, `drop_data`,
  `finish_drop`, `dragging`; events `DragEntered`, `DragMoved`, `DragLeft`, `Dropped`,
  `DragEnded`. New icons: `Cut`, `Pencil`, `Paste`; `ui::icon_coverage`; `Field::select`.
* **Compositor**: smithay's drag, with the pointer's focus already on the window the ray meets at
  the point it meets it; the dragged thing drawn as a card hanging from the pointer
  (`Scene::draw_dragged`, `scene::dragged_picture`) in `backend_drm.rs` (Deck) and
  `spatiand-android/src/android/backend.rs` (Beam Pro and the Mac, which builds that file), and in
  the snapshot's room picture while a drag is held.
* **Apps**: SpatiShelf offers `application/x-spatiand-places`, `text/uri-list` (a remote's files by
  their WebDAV address) and text, and takes drops onto the folder under them; SpatiMesh takes files
  and links; SpatiQuill types dropped files' paths, quoted.
* **Not yet**: drags out of a web page (WebKit's own), and into or out of the Mac's own windows or a
  host's.

## Checked here, and how

| | |
|---|---|
| WPE on Linux (`SYSTEM_DEPS=1`) | built; `spatimesh-wpe`'s 21 engine tests pass, four runs in a row (`SPATIAND_WPE=$WPE_WORK/out/linux EGL_PLATFORM=surfaceless cargo test -p spatimesh-wpe --test engine -- --test-threads=1`); its extensions test under `xvfb-run` |
| SpatiMesh in the compositor | `tools/spatimesh-ui-e2e.py`: every check, on WebKitGTK and on WPE (`SPATIMESH_E2E_ENGINE=wpe`) |
| Drag and drop | `tools/spatiand-dnd-e2e.py` (new, in CI): SpatiShelf to SpatiShelf (moved, the folder lit), SpatiShelf to SpatiMesh's tabs (opened), the card in the room |
| SpatiShelf | in the snapshot compositor: menus, rename in place, drags onto a folder and the side, the band, Move to...; unit tests |
| Unit tests | spatimesh (lib and bins), spatishelf, spatiquill, spatiand-app, spatiand |

## Not checked: to try by hand

* Everything on the Mac: 0002--0006 applied there, 0004 above all; drag and drop on the Mac's room
  (the card is drawn by the Android backend file the Mac builds, not by anything run here);
  SpatiShelf's Control-click menu.
* The SteamOS bundle (build it in the 22.04 root; run SpatiMesh on the Deck with `wpe/` beside it).
* Android: the NDK build of WPE, the APK with it, pages drawing on the Beam Pro.
* In the glasses: the drag card's size and place; dragging with the pads; SpatiShelf's
  press-and-hold menu.
* Preconnect's effect, measured.

## SpatiQuake

A new repository, [peprasetya/SpatiQuake](https://github.com/peprasetya/SpatiQuake): vkQuake's
history with `spatiquake/PLAN.md` on top. A cloud session of its own builds it: vkQuake with an
OpenXR mode through Spatiand's runtime (`crates/spatiand-openxr`, `docs/openxr.md`), so the same
build runs on the Deck and through `spatiand-host`, with LibreQuake's free data. Its progress is in
that repository's `spatiquake/STATE.md`.
