# SpatiMesh: where the work stopped (handoff notes)

For the next session picking up SpatiMesh. The full design and status table is
docs/spatimesh.md; this is what a new session needs on top of it.

## State

* All work was on branch `spatiand-mesh`, merged into `master` at the end of the cloud session.
* CI (3 jobs: linux, mac, android) green on the last commit (run 31): WebKitGTK and CEF run the
  shared extension end-to-end test (Chrome MV3 + Firefox MV2 extensions, plus a
  declarativeNetRequest rule blocking a fetch); the Mac job runs the same test on WKWebView;
  the Android job compiles the Java and runs WireTest and DnrTest.
* Order of the original plan: (a) doc, (b) trait + engine-free parts, (c) WebKitGTK, (d) Mac,
  (e) Android, (f) SteamOS (Steam's own CEF, never WebKitGTK there), (g) extensions, (h) sync.
  All done except what is listed under "Open".

## User's standing rules

* No Flatpak, ever: Spatiand must be independent and easy to install.
* SteamOS uses Steam's Chromium (CEF); WebKitGTK only on other Linux.
* Extensions built from the ground up after Chromium's and Firefox's concepts (manifest,
  injection, backgrounds, APIs); where an engine lacks support, build it into the engine.
* Media: Spatiand's own FFmpeg (widen it if narrow) for H.264, H.265, AV1 (hardware), all
  audio codecs, MP4/WebM/MKV, ideally AVI/DivX/Xvid; SpatiReel uses the same FFmpeg.
* Push after every working step (never ~30 min unpushed); after each push read the 3 CI jobs
  and fix red before adding more. Commit explicit paths, never `git add -A`.
* If stuck after two attempts: write the blocker in docs/spatimesh.md, push, tell the user.
* Every report says what could not be checked and what to test by hand.

## Open

1. Blocking webRequest on every engine (only declarativeNetRequest applies requests today).
2. WebKitGTK: code from the window reaches only the top frame (needs a web-process extension).
3. Android: code reaches only the top frame; nothing of extensions has run on a device
   (Beam Pro) yet: check Boundary.java finds DOCUMENT_START_SCRIPT and WEB_MESSAGE_LISTENER,
   blocked requests (Dnr.java's unreadable body), redirects, resource-type guessing.
4. Mac: only the end-to-end test has run; everything else by hand.
5. Task #11: the window's extension UI (toolbar buttons, badges, popups, options) end to end.
6. Task #14: media through Spatiand's FFmpeg in SpatiMesh.
7. Sound placed per window.

## Checking locally (Linux)

* `cargo test -p spatimesh --lib`, `--test api_js`, `--test android_js` (Node).
* `xvfb-run -a cargo test -p spatimesh-webkitgtk --test extensions`.
* CEF builds need CEF headers: `CEF_ROOT=<cef> CFLAGS=-I<chromium src> cargo build -p spatimesh-cef`;
  its end-to-end test runs in CI.
* Mac ObjC (webkit.m) was checked with clang `-fsyntax-only` against stub headers; CI's Mac job
  is the real check.
* Android Java: `javac --release 17` against android.jar (or Robolectric's android-all);
  `javac Dnr.java DnrTest.java && java id.prasetya.spatiand.mesh.DnrTest`.

## CEF reference rules learned the hard way

* A ref-counted struct passed to a CEF function as an argument gives up one reference: do not
  release it afterwards (`SHARE()` it if kept). Returned objects carry a reference to release.
* Never NULL in `execute_function` argument arrays.
* Alloy rewrites unknown `chrome-extension://` to `chrome-extension://invalid/`: Chrome
  extensions live at `spatimesh-extension://` on CEF (`Addresses::Chromium`).
* Mac shared memory names must be at most 31 bytes.
