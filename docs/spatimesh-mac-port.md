# SpatiMesh on the Mac: the Deck's Chromium, a client of Spatiand

The owner, 2026-11-10: "spatimesh is a spatiand app. it should be run on spatiand, because the
protocol we use is wayland ... make the same as deck", and "even in desktop mac if called the
spatimesh it should be run over spatiand". So on the Mac SpatiMesh is what it is on the Deck and the
Beam Pro: Chromium's Linux browser -- Aura, Views, Ozone with its Wayland platform -- talking Wayland
to Spatiand. In the room Spatiand draws it as a window of the room; on the Mac's desktop (2D mode)
Spatiand shows it in a Mac window, as it shows SpatiQuill and the others (DeskWindows). Chromium's
own Mac browser (Cocoa windows) is never SpatiMesh.

## How

As the Beam Pro got it: the Deck's browser built for another system. The Beam Pro's build keeps
`target_os = "linux"` and swaps the libc (bionic, from Termux's patches: `android-libc/`). A Mac
cannot do that -- its binaries are Mach-O, linked by Apple's ld against the macOS SDK -- so here it is
`target_os = "mac"` with the Linux UI switched on: `use_ozone`, `ozone_platform = "wayland"`,
`use_aura`, `toolkit_views`, in `out/mac-ozone` beside the toolchain check in `out/mac`.

What that takes, in order:

1. **GN**: the Linux-only guards around Ozone/Wayland and what it needs, widened to the Mac. Done so
   far on littleotter (not yet a patch): `ui/ozone/platform/wayland`, `third_party/wayland`,
   `ui/gfx/linux`, `ui/base/ime/linux`, `build/config/linux/libdrm`, `third_party/minigbm`,
   `ui/linux`, `third_party/libdrm` (`assert(... || is_mac)`). Next: targets defined only under
   `if (is_linux ...)`, first `//ui/base/clipboard:clipboard_util_linux` (D-Bus): the Beam Pro's
   patches already make the Wayland clipboard work without D-Bus -- reuse them, as everything of the
   Beam Pro's that is not bionic's alone.
2. **Compiling**: libwayland and xkbcommon on macOS (no `SOCK_CLOEXEC`/`MSG_CMSG_CLOEXEC`, no
   epoll in the client; the Mac already builds xkbcommon for the room), `memfd` for shared memory
   (`shm_open` instead), and every `BUILDFLAG(IS_MAC)` path in `chrome/`, `content/` and `ui/` that
   assumes Cocoa windows, sent the Linux way when the UI is Ozone.
3. **Frames to the room**: as the Beam Pro hands the room Android hardware buffers through a socket
   posing as a dmabuf plane (`android-libc/0021`), the Mac hands IOSurfaces, which the Mac's room
   already takes from its own applications (`crates/spatiand-surfaces`). Shared memory first.
4. **Spatiand's data** (patches 0008-0016, 0020-0022) built on the Mac as on Linux: `mac/0002`
   left the profile menu's lock buttons out behind `IS_LINUX` -- a workaround until this.
5. **In Spatiand.app**: the program and its frameworks inside the bundle, started by the room as on
   the Deck (`crates/spatiand/src/own.rs`, `chromium_spatimesh`), FFmpeg found in the bundle
   (`mac/0001`).

## Where it stands (2026-11-10)

`out/mac` (Chromium's Cocoa browser, `mac/0001`-`0002`, patches 0001-0022) builds and loads a page:
the toolchain, Spatiand's FFmpeg and the patches compile for macOS. It is not SpatiMesh and is not
put in Spatiand.app. The stale Rust SpatiMesh was moved out of littleotter's Spatiand.app
(`~/Developer/spatimesh-rust-removed-2026-11-10`): until this port, the Mac's launcher has no
SpatiMesh.

**Later the same day (on littleotter, not yet built):** GN configures the Linux UI on macOS
(`out/mac-ozone`, 32,458 targets): the guards above widened, the Beam Pro's "Wayland's clipboard
without D-Bus" applied with its guard widened to the Mac, and AppKit's pasteboard kept for the Cocoa
UI alone (`is_mac && !use_ozone`, in `ui/base` and `ui/base/clipboard`). The Linux-only sources
Chromium's DEPS leaves out of a Mac checkout (libwayland, wayland-protocols, libdrm, minigbm, libsync,
lss, fontconfig, cros_system_api) were copied from the Deck's tree of the same version. Chromium's
own libwayland (`third_party/wayland`, compiled into the program as on the Deck and the Beam Pro, see
CLAUDE.md) made to build on macOS, each change behind `__APPLE__`: Chromium's `config.h` says what
macOS has; sockets made close-on-exec by fcntl and with `SO_NOSIGPIPE` (no `SOCK_CLOEXEC`, no
`MSG_NOSIGNAL`); `MSG_CMSG_CLOEXEC`'s own fallback; the BSD peer-credentials branch; no epoll (the
server half's alone). Its source is outside Chromium's git: kept as a diff beside the patches once
built, as the Beam Pro's ANGLE change is. The build: `~/Developer/spatiand-chromium-mac/ozbuild.sh`
(siso, `-k 0`), at night when the owner says.

## The method (2026-11-10, the owner's choice)

**One condition names the port**: `BUILDFLAG(IS_MAC_LINUX_UI)` in C++ (`build/build_config.h`:
macOS with Ozone) and `is_mac_linux_ui` in GN (`build/config/BUILDCONFIG.gn`, the arg
`spatiand_mac_linux_ui = true` in `out/mac-ozone/args.gn`). The rule: **the UI follows Linux's
path, the system below it stays macOS's** (processes, memory, files, sandbox, keychain, fonts
through CoreText).

Chromium's UI code has thousands of Linux/Mac switches (`chrome/browser` alone: 4,741 `IS_MAC`,
2,612 `IS_LINUX`). **The FreeBSD port of Chromium is the map** (FreeBSD ports `www/chromium/files`,
156; fetched to `/media/shared/Linux/App/freebsd-chromium-map`): it builds the Linux UI on a BSD
kernel, macOS's family, and widens Linux checks to BSD wherever that is the right path there.

- `third_party/chromium/mac/bsd-map.py`: its C++ widenings, in the UI's directories only, made with
  `IS_MAC_LINUX_UI`, each placed by its hunk's context. First run: 425 in 242 files; 18 behind an
  earlier Mac branch are reported, not forced (fonts, accessibility, accelerator labels).
- `third_party/chromium/mac/gn-ui.py`: FreeBSD makes GN's `is_linux` true, which the Mac cannot
  (`is_mac` drives its toolchain), so the UI's build files (`ui/views`, `chrome/browser/ui`,
  `apps/ui`) say `is_linux || is_mac_linux_ui` and `is_mac && !is_mac_linux_ui`; asserts keep
  plain `is_mac`. `content/browser` is done by hand: it holds the Linux zygote and sandbox, which
  must never come to the Mac.

## Where it stopped (2026-11-10, 18:20)

Third pass (`logs/build-ozone4.log`): 144 failures, read and sorted. Fixed since, not yet built:
a fault of `gn-ui.py`'s first version (its assert fix-up spanned lines and undid one real rewrite
after each assert: `ui/views`' Cocoa block among them; rewritten again, 40 files); the Linux UI's
strings and resources (`IDS_`/`IDC_`/`IDR_` missing: GRIT's `is_linux` is now true where
`is_mac_linux_ui`, beside `is_macosx`); the map's edits to `content/browser` (process
management, system) and the certificate manager (NSS on Linux, the Keychain on the Mac) put
back. GN configures. **Next**: run `ozbuild.sh` (a long pass: only when the owner says), then
the remaining Cocoa sources outside `ui/views`/`chrome/browser/ui` (`content/app_shim_remote_cocoa`,
`components/remote_cocoa`, `chrome/browser/*_mac.mm` beside the UI, `content/browser`'s views by
hand), and `JavascriptAppModalEventBlockerAura`.

## Evening of 2026-11-10 (passes 5 and 6)

Pass 5 stopped after 17 failures: the UI's strings (GRIT) and the WebUI pages, which everything
waits on. Fixed:

- **GRIT reads a `.grd` both ways** (`tools/grit/grit/node/{base,misc}.py`, `xtb_reader.py`): what
  macOS alone takes, plus what the Linux UI adds; where both define the same resource (the same
  string worded for each system: fonts, proxy settings, the certificate store), macOS's stands. The
  C++ takes both the `IS_MAC` and the `IS_MAC_LINUX_UI` paths, so it needs both sets of strings.
- **WebUI pages keep macOS's reading** (`tools/grit/preprocess_if_expr.py`): they are web content,
  and their handlers are macOS's.
- **libwayland**: `ppoll()` by `poll()` on macOS. **libffi** is macOS's own (in the SDK, as the
  Deck's is SteamOS's). **libdrm** builds (no `sysmacros.h`). **GBM off** (`use_wayland_gbm =
  false`), **no libsync** in the Wayland platform and no dma-buf fences on Apple (as FreeBSD's port).
- **Fonts stay CoreText**: the map's fontconfig widenings in `ui/gfx` were put back, and
  `bsd-map.py` now leaves `ui/gfx/font*` out.
- **The GPU's images stay macOS's** (`gpu-mac.py`): Chromium's shared-image code takes
  `#elif IS_OZONE` before `#elif IS_APPLE`; on macOS the IOSurface backings win, and the Ozone
  (dma-buf/Vulkan/Dawn) backings are not built. What the window shows still goes through Ozone.
- The UI's theme is Aura's (`ui/native_theme`); the Cocoa menus, the Mac's snapshot and the
  bookmark pasteboard go Aura's way (`gn-ui.py ui/menus ui/snapshot components/bookmarks/browser`).
- **Workaround, named**: the Aura property holding a view's accessible parent is left out on Apple
  (macOS's `NativeViewAccessible` is an owning NSAccessibility wrapper a window property cannot
  hold). So SpatiMesh on the Mac has no accessibility linkage to macOS; the proper alternative is
  the room's own channel (`describe` in `spatiand_xr_v1`), as on the Deck, where there is no AT-SPI
  bus in the room either.

**Passes 7 and 8 (the same night)**, the Cocoa UI taken out, the system kept:

- `content/browser`: the Mac web-contents and render-widget views, drag, Cocoa input and key
  equivalents, the CoreAnimation compositor view, `app_shim_remote_cocoa` out (a block of their
  own under `!is_mac_linux_ui`); sandbox, process launching, speech, screen lock, theme helper,
  file watching kept. macOS's Now Playing kept with no app shim (`ApplicationHost::GetForNativeView`
  answers none). The text-input host (Cocoa's input method) is not bound: **gap**, a page asking
  for it would lose its renderer.
- `chrome/browser`: the app controller and menu bar, Cocoa key equivalents, the Cocoa views'
  delegates, `chrome/browser/ui/cocoa` out. `ChromeBrowserMainPartsMac` stays for the system
  (keychain, relaunch, disk-image install, app shims, trust store) without its Cocoa pieces, with
  Ozone's extra parts, the Deck's views parts (`ViewsLinux`) and Spatiand's own parts.
- `remote_cocoa/app_shim` builds only `CrApplication`, the process's NSApplication.
- WebUI pages (`chrome/browser/ui/webui`) keep the Mac's: 40 files of the map's widenings put back;
  both scripts leave the folder alone.
- GRIT: in the Linux UI's reading `is_macosx` is false, as on Linux, and the Mac-first rule looks
  through nested `<if>`s.
- viz: no overlays on the Linux UI on macOS (the window goes through Wayland).
- Aura's views answer Cocoa's own window ideas (activation independence, fullscreen Spaces,
  event-tracking loops) with nothing.

**To come over from the Beam Pro** (its patches that are not bionic's alone): Spatiand's own file
chooser (`android-libc/0019`; the Linux path has no portal on the Mac). Each window's sound handed
to the room (`android-libc/0012`) came over on 2026-11-11, below.

## It runs (2026-11-11, night)

Pass 13 links; from there each fault was found by running it (`tools/spatimesh-mac-test`, the
snapshot room on littleotter: no glasses, no screen, no unlocked Mac needed):

- FFmpeg and the OpenSSL it links are put in the framework's `Libraries` after every build
  (`third_party/chromium/mac/ozbuild.sh`).
- The global-shortcut listener answers none on the Mac (the Ozone one fell off its end and trapped).
- `fullscreen.allowed` is registered on the Linux UI (BrowserCommandController reads it).
- The renderer's request for Cocoa's text-input host is taken and let go (it ended the renderer).
- viz draws through Ozone's canvas (wl_shm), not a CALayer: `output_surface_provider_impl.cc`.
- A page's Mac scrollbars are painted with macOS's theme (`NativeThemeMac::GetInstanceForWebScrollbars`):
  the UI's theme is Aura's, which has no Mac scrollbar parts, and a page that grew a scrollbar
  ended its renderer (SIGABRT, "Aw, Snap! 6").

**Sound**: the Beam Pro's sound manager (`media/audio/spatiand`) in place of CoreAudio; the audio
service's seatbelt may connect to the room's two sockets (`spatiand-(sound|mic)`) and nothing
else; patch 0007's routing on events; each window says its sound key (patch 0020), which the room
uses where it cannot read the client's process. Checked in the snapshot room with its sound on
(`SPATIAND_SNAPSHOT_SOUND=1`): each window's sound through its own slot; a tab moved into another
window (the oldest, so not the default) heard there; three media tabs in two windows, each at its
window.

**Codecs** (`media/matrix.html`, each file 3 s): H.264, HEVC, VP9, MPEG-2 video; AAC, AC-3, MP3,
Vorbis, FLAC; MP4, WebM, MKV (with SRT), MPEG-2 TS (H.264, HEVC, MPEG-2), AVI (DivX/MP3) all play.
**Not**: AV1 (no decoder: `enable_dav1d_decoder = false` in the shared args), VOB (no MPEG-PS
demuxer, as on the Deck), MP2 sound (Chromium has no MP2, as on the Deck: the picture plays
silent), Opus in a plain file (`<video src>`: FFmpeg's demuxer path rejects the track; Opus through
MSE, which is YouTube's path, plays and is heard). Probably the same on the Deck; not checked there.

**Not yet**: drawing with the GPU (all of the above is software, `--disable-gpu`); SpatiMesh
started by Spatiand.app (own.rs) from its bundle; Spatiand's own hooks that are `IS_LINUX`-only
(profile lock, ProfileServiceFactory, SpatiKey's entry points, the profile menu); the file chooser.

## Resuming (littleotter is shut down at night)

Everything changed in its tree is kept here: `third_party/chromium/mac/save-wip.sh` (run on
deepmagpie after each step) writes `mac/wip/linux-ui.patch` and `mac/wip/libwayland.patch`. After a
shutdown nothing needs restoring: run `ozbuild.sh` again (under `caffeinate -i -s`), and the build
goes on from what it had. Only if the tree were lost: a fresh Mac checkout, `git apply
mac/wip/linux-ui.patch`, and `patch -p1` the libwayland one in `third_party/wayland/src`.

## Morning of 2026-11-11: the GPU, the keyboard, the media

- **Keyboard**: keys arrived (keydown with the right key and modifiers) but no text was typed.
  Two causes: `use_xkbcommon` is Linux-only in GN (now on, linking the room's own
  `mac/xkbcommon`, as the Beam Pro links the compositor's; headers kept in
  `third_party/spatiand_xkbcommon/include`, the library copied in by `ozbuild.sh`), and
  `ui/base/ime/init/input_method_factory.cc` took `IS_APPLE` before `IS_OZONE`, giving the Cocoa
  `InputMethodMac`, which never inserts text. Typing into pages, the address bar and the profile
  lock's PIN tested. Shortcuts are the Mac's (Cmd+L etc.).
- **GPU**: Chromium's own Mac presenter draws each frame into an IOSurface (Graphite on Dawn/Metal).
  With remote CoreAnimation off (`ui/base/cocoa/remote_layer_api.mm`: no CALayer host in the Linux
  UI) and macOS's overlay processor (the primary plane, one layer), `CALayerParams` carries the
  frame's IOSurface; `viz::HostDisplayClient` hands it to `OzonePlatform::ShowIOSurfaceFrame`,
  and `ui/ozone/platform/wayland/host/spatiand_iosurface_handover.cc` gives it to the room as
  every Spatiand application on the Mac does (`spatiand-surfaces` + the `SPSF` placeholder of
  `spatiand_proto::handover`), attached once per size through Ozone's frames, the name rewritten
  each frame. Nothing copied. `--disable-gpu` is gone from `own.rs`. The room now reads a client
  IOSurface in its own pixel format (Chromium's are RGBA). Software mode keeps the stub overlay
  processor and the Ozone canvas.
- **SpatiKey** on the Mac listens in `~/Library/Application Support/Spatiand/spatikey.sock`
  (spatikey-core); `components/spatikey/client.cc` looked in the runtime folder, so the locked
  profile had no fingerprint button and logins none of SpatiKey's. Fixed and tested.
- **Media** (patch 0023, all platforms): H.264/HEVC in MPEG-TS gave VideoToolbox nothing but
  end-of-stream (the Annex-B converter dropped every packet of a stream already in Annex B);
  Opus in a file was dropped (Chromium's list names `libopus`, Spatiand's FFmpeg has `opus`);
  MP2 is let in and read as MPEG audio; VOB gets FFmpeg's generated PTS. With the GPU, on the
  M6: AV1 (VideoToolbox; still no software AV1, the owner's choice), H.264, HEVC, VP9, MPEG-2,
  TS, VOB, AVI, MKV+SRT, Opus, Vorbis, FLAC, AAC, AC-3, MP3, MP2 all play. The earlier MP2 test
  file was mono at 384 kbit/s, which Layer II does not allow; Symphonia refuses it, rightly.
- Sound per window and the tab move re-tested with the GPU: as before.
- Rig: `go-gpu.sh` (run.sh without `--disable-gpu`), `cdpmedia.py PORT URL` (a page's media log
  over DevTools), `keys.html`, `matrix.html?f=a,b`.
- **Trap**: rebuilding and re-signing Spatiand.app made macOS ask for Screen Recording again
  (`universalAccessAuthWarn`), although the certificate and designated requirement are the same.

## Night of 2026-11-11: an agent to macOS, the tab drag, SpatiKey, the chooser

- **No Dock icon.** The browser's bundle is `chrome/app/app-linux-ui-Info.plist` (`LSUIElement`; no
  document types, URL schemes, AppleScript or Handoff, so macOS does not offer it as a browser), and
  `RegisterBrowserCrApp` sets the accessory policy. Chromium told its helpers from the browser by
  `LSUIElement`: the browser's plist now says `SpatiandBrowserProcess`
  (`base::apple::IsBackgroundOnlyProcess`). `lsappinfo` gives `type="UIElement"`.
- **A tab dragged off its strip** started Chromium's window drag (a `wl_data_device` drag of
  `chromium/x-window-drag`, in a nested loop), and `wayland_event_watcher_mac.cc` enabled its
  one-shot read callback again only after dispatching, so inside that loop nothing was read: the tab
  was gone, no window came, the browser could not end. Enabled before dispatching now. The Deck's
  watcher is persistent and never had this.
- **Quitting as on Linux**: `SetTryingToQuit` and the downloads-in-progress question
  (`application_lifetime.cc`, `browser_close_manager.cc`) were left to Cocoa's AppController.
- **SpatiKey** as on the Deck: `device/fido/spatikey` built and the platform authenticator (before
  macOS's Touch ID keys and iCloud Keychain, which are not offered), the passkey dialog's Linux
  branches, `SpatikeyPasswordStoreBackend`; Password Manager opens `Spatiand.app/Contents/MacOS/spatikey`.
- **Files** are chosen in Spatiand's chooser (`android-libc/0019` brought over):
  `Spatiand.app/Contents/MacOS/spatiand-choose`, which `mac/make-app.sh` does not carry yet.
- Rig: `sk.sh` (the stand-in SpatiKey service), `menus.html`, `ext-parts`, `SM_APP`; the rig runs
  with `--use-mock-keychain` (over ssh the login keychain is locked). A copy of the bundle runs only
  from a folder named `SpatiMesh.app`.
- **Open**: a page's `<select>` list opens at the page's coordinates taken as the window's (Cromite's
  viewport protection gives the page a view at 0,0; seen on the Beam Pro too); every exit ends in a
  trap (three dangling pointers at teardown, which Cromite's build checks); the Mac keeps Chromium's
  key in the Keychain where the Deck and the Beam Pro run `--password-store=basic`.
