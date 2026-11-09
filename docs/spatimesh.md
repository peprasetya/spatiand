# SpatiMesh, Spatiand's web browser

A full browser in the class of Safari and Chrome, written as one of Spatiand's own applications
(docs/spatiand-apps.md): a window in the room drawn with the SDK and the room's widgets, the same
program on the Mac, the Deck, other Linux and the Beam Pro. The page itself is drawn by the
platform's own web engine, behind one Rust interface. Asked for on 2026-10-16; this is the plan,
and at its end what has been built of it.

| Platform | Engine | Where it comes from |
|---|---|---|
| Linux, the Deck | **Chromium** with Cromite's patches and ours (`third_party/chromium`) | `~/spatiand-chromium`, packaged on its own |
| Android (Beam Pro) | **Chromium, the Deck's**, built for Android's libc (2026-11-08; "SpatiMesh for Android" below) | in the APK, `libspatimesh_chromium.so` and its resources |
| Mac | **Chromium, being ported** (`CHROMIUM_TARGET=mac`, docs/mac-plan.md); none until then | -- |

**WPE WebKit and GStreamer removed on 2026-11-10** (the owner's decision: Chromium has replaced
WPE on every platform, and GStreamer was WPE's media alone). Gone: `third_party/wpe` (its build,
WebKit patches and GStreamer sink), `crates/spatimesh-wpe` (the engine program) and
`crates/spatimesh-shim` (its C interface), WPE's place in the APK, the Mac app and the Linux
release, and the end-to-end runs that drew pages through it (`tools/spatimesh-e2e.py`,
`spatimesh-ui-e2e.py`, `spatimesh-sync-e2e.py`). What Chromium for Android had taken from WPE's
Android build -- libffi, SQLite and xkbcommon -- is built for it now (`android-libc/deps.sh`,
`android/xkbcommon`). The window program below stays: its stores are the formats Chromium's patches
keep, and with no engine program it draws pages with the stand-in (`fake`), as the drag-and-drop
run does (`tools/spatiand-dnd-e2e.py`). The WPE sections below are the record of what it did.

**Changed on 2026-10-30:** the owner moved SpatiMesh to **Chromium with Cromite's patches and
ours**, keeping Chrome's own browser layer (Chrome's extensions, as Chrome has them) with
SpatiMesh's look over it, as Vivaldi does. WPE's newest code kept failing on real sites (its Skia
compositor's work for `backdrop-filter` grew as 2^n with the number of glass panels -- fixed
upstream on 2026-10-18, after 2.54.1 -- and a reflection's mask before that), and on the Mac our
port of it has one user. Ours to add: drawing through Spatiand's own Wayland protocol on every
platform (Chromium's Linux drawing, Ozone/Wayland, taken to the Mac and Android), media on
Spatiand's FFmpeg, SpatiKey's passwords and passkeys, profiles synced through spatiand-host. The
first build (Chromium 153.0.8010.37, Cromite's 327 patches and its Linux arguments) is for the
Deck, where Ozone/Wayland already draws into the room. WPE stays the engine of the window
described below until Chromium replaces it.

**On 2026-10-30, Chromium became the room's SpatiMesh on Linux** (`~/spatiand-chromium`, the launcher's
SpatiMesh; WPE everywhere else until Chromium is ported) and took Spatiand's data over. See "Chromium on
Spatiand's data" below.

**SpatiMesh for Android (started 2026-11-08) is the Deck's**, not Chrome's Android application: Chromium's
Linux browser with every patch here, built for Android's libc and carried in Spatiand's APK as
`libspatimesh_chromium.so` beside the other programs, a Wayland window of the room like them (docs/spatiand-apps.md, "Android processes").
Android's Spatiand gives its programs the Deck's folders and sockets ($HOME, $XDG_CONFIG_HOME, $XDG_RUNTIME_DIR in
the APK's own storage), so Spatiand's data, the profile locks and SpatiKey work as they do on the Deck. Termux builds
Chromium the same way (`x11-packages/chromium-host-tools/cr-patches` in termux-packages: the Linux build pointed at
Android's libc, `--target=aarch64-linux-android`), which is where its changes for that libc come from. Still to write
for Android: the sound of each tab to the room's `spatiand-sound` (as the other programs send theirs there), video on
Spatiand's FFmpeg with Android's decoders in place of VA-API, the drawing through the GPU (software, through `wl_shm`,
to begin with), and the sandbox (seccomp only: Android lets no app make namespaces). The tree is on deepmagpie,
`spatiand-chromium-android`, fetched for both targets; Chrome's own Android application (`CHROMIUM_TARGET=android`)
is not built.

**It runs (2026-11-08).** The first build linked as an arm64 program of Android's (`/system/bin/linker64`), and on
the Beam Pro, headless, started from adb as Spatiand's own app user (`run-as`, from its cache), it fetched
example.com and Wikipedia and drew Wikipedia's page right, in Android's fonts through Chromium's own fontconfig.
Names went by DNS over HTTPS to OpenDNS, Cromite's default, and never to the 8.8.8.8 of Termux's change. Learnt:
its libc++ must be the one of the NDK it was built against, Chromium's, newer than the r28c one the APK carries
(`std::__hash_memory`), so it is to be linked in whole; libdbus looks for a system bus at its build folder's path.
Not yet: carried in the APK, a window in the room, sound, video, the GPU.

**In the APK (2026-11-08).** `third_party/chromium/build.sh package` (with `CHROMIUM_TARGET=android`) makes
`spatimesh-android.tar.gz`: the program stripped (283 MB), the NSS and NSPR it brings, Chromium's minigbm, and its
resources. `android/build.sh` takes it from `~/Developer/spatimesh-android` (or `SPATIAND_CHROMIUM_ANDROID`): the
program and libraries into the APK's library folder, the resources, `run-android.sh` (as `run.sh`) and the fonts'
setup into its assets, stamped with the release and the program's checksum. The app unpacks them into
`files/spatimesh-chromium` at start, again only when the stamp changes, the stamp last; once it is there the
launcher's SpatiMesh is Chromium (`crates/spatiand/src/own.rs`; the launcher re-reads its applications each time it
opens, as on the Deck), started through `/system/bin/sh run.sh`, as nothing in the app's files may be run itself.
Its libc++ is linked in whole (`android-libc/0011`) and it finds its resources by `SPATIMESH_ASSETS`
(`android-libc/0010`). Different from the Deck, each by what Android has not got, each to be closed:

* **The sandbox is Chromium's second layer alone** (`android-libc/0013`, 2026-11-08). Termux runs Chromium with
  `--no-sandbox` (its 1001; an earlier line here wrongly said the filter was on); SpatiMesh keeps the seccomp filter
  on every process, a tab reaching files only through the browser's broker, as on the Deck. The Deck's first layer
  (namespaces, or the setuid helper) Android lets no app have, so the zygote starts without it. What bionic does that
  glibc does not is allowed as for Chrome on Android (thread clone flags, prctl options, memfd seals, PI futexes,
  per-thread clocks); Chrome on Android's own baseline is not used, as it lets a tab open files and leaves them to
  SELinux, which here would be the room's own domain. bionic reads `/proc/self/stat` for the main thread's stack on
  every `pthread_getattr_np()`, which a sandboxed tab may not, and Blink stops the tab when that fails: it is asked
  once before the sandbox and answered from then on (`base/spatiand_main_thread_stack.cc`). `chrome://sandbox` on the
  Beam Pro: Seccomp-BPF yes, with TSYNC; layer 1 none. Tried sandboxed: Wikipedia, BBC, GitHub, YouTube, a WebGL
  page, and a 720p H.264 film (184 frames in 6 s, none dropped).
* **It draws with the GPU** (2026-11-08, `android-libc/0014`): ANGLE, linked into the program, on Android's own
  Vulkan loader (`angle_use_custom_libvulkan = false`; on Linux ANGLE looks only beside the program for one of
  Chromium's) and so on Adreno's driver: on the Beam Pro `chrome://gpu` says compositing, rasterization and WebGL
  hardware accelerated, `ANGLE (Qualcomm, Vulkan 1.1.128 (Adreno (TM) 710))`. Android's EGL and Vulkan make no
  Wayland surfaces, so each frame is drawn off screen.
* **Its frames reach the room without a copy** (`android-libc/0021`, `android/eglshim`): each is copied on the GPU
  into an Android hardware buffer, the buffer sent once to the room through a socket that stands as a dmabuf's
  file, under Spatiand's own modifier (`0xf500000000000001`), which no real dmabuf has. The room's dmabuf global
  offers RGBA and RGBX with that modifier, and when Smithay asks EGL for the image, the room's EGL shim reads the
  buffer from the socket and makes the image through `EGL_ANDROID_image_native_buffer`: nothing is copied in memory
  on either side. ANGLE takes such buffers as EGLImages on Android's libc (its own
  `linux/AHardwareBufferImageSiblingVkSpatiand`, as its Android build does; Vulkan's import from Android's buffers
  turned on); the buffer allocator's mapper is loaded before the GPU sandbox, which keeps the vendor's libraries
  out. Each frame's fence (a sync file) goes down the buffer's socket, which the room keeps, just before the frame is
  committed, and the room's GPU waits for every fence that has come before it draws (`android-libc/0024`,
  `crates/spatiand-android/src/android/explicit_sync.rs`): Android's buffers carry no fences of their own, and
  Chromium's Wayland host gives fences only through `wp_linux_drm_syncobj_v1`, which needs DRM. Where the room
  offers no such modifier the frames are read back into shared memory as before (`0014`), now as RGBA through a
  pixel buffer (`0020`): asked for BGRA, ANGLE had converted one pixel at a time, 190 ms a frame. Measured in the
  Beam Pro's room (`tools/beam-stand-in`, below), a full-window canvas animated every frame: 4.6 frames a second
  read back as at first, 23 read back as RGBA, 55 to 57 in hardware buffers with the room presenting 60. Scrolling
  Wikipedia by script, 32 frames a second while Chromium waited for its own fence (on its GPU process's main thread,
  behind 10 ms of tile raster), 52 with the fence handed to the room; by wheel, about 43. What is left is that
  raster: the Beam Pro's GPU paints new tiles slower than the Deck's. WebGPU is off as on every platform (Cromite's
  `Disable-WebGPU`, its runtime feature forced off).
* **The GPU process is sandboxed too** (`android-libc/0016`): Chromium's Linux GPU sandbox gave up because Adreno's
  driver has started threads by then; here the filter goes on every thread at once (TSYNC), the broker is given
  what the driver, Android's Vulkan loader and MediaCodec open as they run, binder (which works only for the process
  that opened it) is opened before the sandbox by one decoder made and let go, and the few calls Android's libc and
  driver make that glibc does not are allowed as Chrome on Android allows them. On the Beam Pro `chrome://gpu`:
  Sandboxed true, everything still hardware accelerated, and H.264, VP9 and HEVC decoded by Qualcomm's decoders
  from inside it. Every session's GPU process ended once, two minutes after it started (exit 5, a CHECK): collecting
  Dawn's information asked for the program's path of `/proc/self/exe`, which the broker does not read; it is asked
  before the sandbox now and remembered (`android-libc/0022`). Android has no D-Bus either, and Chromium no longer
  tries one at every turn (`0023`).
* **Sound, as on the Deck, through the room** (2026-11-08, `android-libc/0012`): there is no PulseAudio, so
  Chromium's Linux audio has a backend of Spatiand's own (`media/audio/spatiand`). Every stream goes to the room's
  `spatiand-sound` under the name of the sink its tab is in, `spatiand.window.<slot>`, which patch 0007 makes the
  tab's output device from `spatiand-sinks` as on the Deck; the room gives each SpatiMesh window a slot keyed by its
  process and surface, writes `spatiand-sinks`, and places the sound at the window through the HRTF, under the
  window's mute and volume. A tab moved to another window has its streams opened again under that window's name;
  a stream for the default device (a tab not yet routed) is heard at SpatiMesh's newest window. The microphone is
  the room's too: `spatiand-mic` (`crates/spatiand/src/own.rs`, `mic`), recording while a page listens, with
  Android's echo cancelling and noise suppression. Tried with a stand-in room on the Beam Pro: a 660 Hz tone came
  through whole under its window's sink, a 7 s tone moved to another window's sink mid-play with nothing lost, and a
  page heard a 440 Hz tone from the stand-in microphone. The room's half (each window's slot) runs only with the
  glasses on, as the room does nothing per frame without them.
* **Video on Android's hardware decoders** (2026-11-08, `android-libc/0015`): Chromium's Linux GPU process decodes
  only with VA-API and V4L2, so here its media client is Spatiand's, MediaCodec through the NDK in the GPU process,
  offering what this machine has a hardware decoder for (on the Beam Pro: H.264 Baseline to High, HEVC Main, VP9
  profile 0, up to 4096x4096; no AV1, which stays off as on the Deck without one; Android's software decoders are
  left to Chromium's own). Each picture, NV12 or I420, is copied into a frame of memory, which reaches the page as
  shared memory and the GPU as a texture: a copy or two more than the Deck's dmabufs. `chrome://gpu` says video
  decode hardware accelerated; 720p H.264, VP9 and HEVC played 146 frames in 5 s, none dropped, through
  `c2.qti.avc.decoder` and `c2.qti.hevc.decoder` (Android's log), and the pictures came out right. Ten-bit video
  (HEVC Main 10, VP9 profile 2, HDR) too (`android-libc/0017`): asked of the decoder as P010, which Chromium's frames
  take as they are, with the stream's colour space and HDR metadata; 720p clips of each played 150 frames in 5 s,
  none dropped, with the colours of the eight-bit decode. MediaCodec reaches Android's media service by binder,
  opened before the GPU process's sandbox (below). A trap for whoever measures this headless: a video alone on an
  emptied page drops about 60% of its frames whichever decoder plays it, software too; on a page with other
  content it drops none.
* No D-Bus: libdbus asks for a system bus at its build folder's path and finds none, as there is none to find.
* **Bluetooth** (`android-libc/0025`, `android/src/id/prasetya/spatiand/BluetoothBridge.java`): Chromium's Linux
  build reaches Bluetooth through BlueZ on D-Bus, which Android has not got. Its adapter here is Android's Bluetooth
  LE, reached through the Spatiand app: an abstract socket, `spatiand-bluetooth`, that only the app's own processes
  may use, carrying one JSON object a line each way (scanning, GATT connections, services, characteristics and
  descriptors read and written, notifications: what Web Bluetooth asks for). Web Bluetooth stays as on the Deck:
  behind "Experimental Web Platform features" (Linux has it as experimental; `--enable-blink-features=WebBluetooth`
  turns it on alone), and a site may ask only once allowed to (Cromite's site setting, blocked by default). **The
  owner's decision (2026-11-09):** Chromium's default for now; at the next monthly update, Web Bluetooth on by
  default on all three platforms (stable in Blink on Linux, Cromite's setting asking; the Mac's when its SpatiMesh is
  Chromium, as WPE has none), noted beside the pinned versions in `third_party/chromium/build.sh`. Tried on the Beam
  Pro with both on: `navigator.bluetooth.getAvailability()` is true, and the bridge asks Android for its Bluetooth
  permission (BLUETOOTH_SCAN and BLUETOOTH_CONNECT, not for location), which the wearer answers on the phone once.
  With it answered, `requestDevice()` scans through Android (its log: a scan started for `id.prasetya.spatiand`)
  and the chooser lists what is near: twelve devices in 20 s, through DevTools' `DeviceAccess`. Connecting and
  reading a device are untried: none near is ours to connect to.
* **Testing in the Beam Pro's room without the glasses**: `tools/beam-stand-in/run.sh start` makes a display from
  adb (the shell may make public ones) that the app takes as the glasses' (`MainActivity.glassesDisplay`, by its
  name): the room draws its world there with a headset that is not there and lets go of the hosts while it lasts,
  as a host shows its windows to one viewer at a time; `run.sh shot` saves what it shows. Its display group sleeps
  with the phone's screen (a locked phone dozes within the minute and the picture is black; the room goes on
  presenting, and frame counts in its log hold): wake it (`input -d <id> keyevent KEYCODE_WAKEUP`, which lights the
  phone's lock screen too) just before a picture. Asleep long enough, Android freezes the app as one in the
  background; bringing it forward (`am start`) thaws it.
* **Choosing files** (`android-libc/0019`, `crates/spatiand-choose`): there is no desktop portal on Android to ask, as
  there is on the Deck, so a page's file input, Save as, a folder to upload or an extension to load ask Spatiand's own
  chooser, a window of the room carried as `libspatiand-choose.so` and run as a program that writes what was chosen, a
  path a line (`--mode open|open-multiple|folder|save`, `--dir`, `--name`, `--accept`). It is SpatiMesh's WPE picker
  made a program: places, the folder's lines (the cursor starts on them, and finding moves it to the first found, so
  Enter takes it), a field to find in or type a path into, several marked then chosen, a name to save under (asked
  before writing over a file). Tried in the Mac's snapshot room by script (a file found and taken, a page saved, two
  marked and chosen) and on the Beam Pro from SpatiMesh: its window opened in the room, a choice reached the page,
  closing it was a cancel. Android's own shared storage (Download, Pictures) is offered, and downloads go to its
  Download folder (`run-android.sh`), only once Spatiand has "All files access", which the app asks for on Android's
  own page (`MainActivity.askForAccess`; android/setup.sh grants it from a computer).
* **Crashes leave minidumps** (`android-libc/0018`): Chromium's crash handler is carried as `libspatimesh_crashpad.so`;
  a tab crashed on purpose (`chrome://crash`) left its `.dmp` in `$XDG_CONFIG_HOME/chromium/Crash Reports/pending`.

**Changed again on 2026-10-29:** WebKitGTK, Linux's fallback, removed too: WPE is the only engine,
and where it is not installed SpatiMesh says so rather than falling back.

**Changed on 2026-10-25:** the first plan used each platform's own engine and bundled none.
WKWebView, Steam's CEF and the Android WebView were all built, then removed
(`spatimesh-webkit`, `spatimesh-cef`, the Android WebView service): one engine, WPE, everywhere,
with media through GStreamer on our FFmpeg. Where the sections below compare WKWebView, CEF and
the WebView, they are the record of what those engines did, kept for why WPE was chosen.

## Chromium on Spatiand's data (2026-10-30)

Chromium keeps its own menus, profile menu, bubbles and settings; what they show and change is what
every Spatiand machine shares. All of it is `chrome/browser/spatiand/` (patch 0009) with a few lines of
hooks, reading and writing the files the WPE build uses, so the two builds and spatiand-host agree:

| What | Where it is shared | Chromium's side |
|---|---|---|
| Profiles: name, colour, order, lock | `apps/spatimesh/profiles/<id>/profile.json` | `profiles_sync`: `default` is `Default`, another is `Spatiand-<id>`; listed without being opened until they are used; added and removed both ways |
| A profile's lock | the same file (salted hash) | `profile_lock_dialog`: its own window, PIN or password, before any of that profile's windows shows; again after its last window closes |
| Bookmarks | `profiles/<id>/bookmarks/<id>.json` | `bookmark_sync`: the bookmark bar is the top level, Other bookmarks the folder `other`; fractional order keys as the Rust side makes them |
| History | `profiles/<id>/history/<machine>-<YYYY-MM>.json`, `forgotten-<machine>.json` | `history_sync`: this browser's own machine id; last 90 days taken in as real visits |
| Logins | `profiles/<id>/cookies/<site>.json` | `cookie_sync`: the same three-way merge as `crates/spatimesh/src/cookies.rs` |
| Search, zoom, restore, bookmark bar, developer mode | `settings/<name>.json`, `profiles/<id>/settings/` | `settings_sync` |
| Chrome extensions | `profiles/<id>/extensions/<ext>.json`, `extension-files/<ext>/` | `extension_sync`: a store extension is fetched on the next machine; one loaded from a folder comes with its files if its manifest has a `key` (which is what keeps its id the same) |

What only this browser needs is in `local/spatimesh-chromium/`, which is never synced. Passwords and
passkeys are not here at all: docs/spatikey.md, "SpatiMesh on Chromium".

**Visited links, light and dark, and the machine's extensions (patches 0013, 0014).** Cromite's "Disable visited pseudo
class" is undone: Chrome's own per-site `:visited` stands, a link followed in one tab updating others of that site
without a reload. Light or dark is `look` in Spatiand's prefs.toml, read every second as its other applications read it,
in place of the desktop portal. What the machine's packages put in `/usr/share/chromium/extensions` (KDE's Plasma
Integration, on a Deck) is not installed.

**The same look on the Beam Pro (patch 0018, 2026-11-09).** The owner found SpatiMesh light there, its
profile's colour beige, its type small, and black about its windows. Patch 0014 hands Spatiand's look to the
toolkits' settings providers, and the Beam Pro's build has no toolkit: the look was read every second and went
nowhere. Where no toolkit is loaded it is now the system's settings itself (a settings provider of its own, in
`ui/linux/dark_mode_manager_linux.cc`); with one, as on the Deck, nothing changes. The toolkit-less UI's type is 13
pixels, the size KDE's toolkit gives it on the Deck, where Chromium's own default is 12; and its fontconfig is set up
as a desktop's (`third_party/chromium/android-fonts.conf`: a pattern naming no family is sans-serif, the families
pages name are the generic ones, type hinted lightly), where it was WPE's file. The black was of two kinds. The band
round every window was the room's: Chromium draws its own frame, with a shadow outside the window it declares (16
pixels each side, 10 above, 32 below), and the room drew whole surfaces opaque. It draws a window's declared geometry
alone now, its pointer and menus counted from it (`window_part` in `crates/spatiand/src/scene.rs`), on every
platform. The blocks inside pages were Chromium's own: re-rastering part of a tile into the tile's existing buffer left
the rest of that buffer as whatever the GPU had there -- black, coloured speckles, the page shifted, a dropdown's list
in the page, in the page's own picture. `run-android.sh` turns partial raster off (`--disable-partial-raster`), which
cured it in the same test: **a workaround**, whole tiles re-rastered where part would do; the fix is to find why a
tile's buffer does not keep its content on this path (ANGLE on Adreno's Vulkan, the Linux shared images built for
Android's libc).

Still open there: a page believes it sits at the screen's corner (`event.screenX - clientX` is 0, its screen
1291x685, its window barely taller than its viewport), so a `<select>`'s list opens at the window's top left rather
than under the element; and YouTube opens, besides its video's routed stream, a short mono one under SpatiMesh's
default name that the router does not reach -- the room now hears it as the newest window's (`own.rs`
`sound::route`), which is right when YouTube is in the newest window. The fix for that one is to route a stream when it
is made, for every frame of the tab, rather than the main frame's every half second.

**Bounce tracking no longer deletes all history (patch 0016).** Cromite's `DATA_TYPE_SKIP_SSL_HOST_STATE` was `1 << 28`, the
same bit as Chrome's `DATA_TYPE_HISTORY`, and bounce tracking adds it to every clearing of a tracker's state: at most
hourly, as soon as a site was due, usually just after a start. History has no per-site filter, so each one emptied the
profile's history and wrote an all-time entry to `forgotten-<machine>.json`. Such entries are from `-9.2e18` to `9.2e18`;
a person clearing all history writes `from` 0. To reproduce, age `timer_last_fired` and the `bounces` times in a copy
of the profile's `DIPS` database and start it: the cleanup runs within seconds.

**A right thumb resting no longer throws the page (patch 0011).** Chromium timed each scroll frame
from the last pointer frame of any kind, so the pointer moving under a resting right thumb made a
scroll's frames look a moment apart, and its end, a quarter of a second after the last of them, look
recent: a fling at a speed worked out from those. It is now timed from the last frame with a scroll
in it. In the test room (`thumbs:600,1,1` in `SPATIAND_STEPS`, on a long page keeping `scrollY` per
frame) the same swipe went 7321 px in 3081 ms before, and 5681 px in 988 ms after, as it does with no
right thumb.

**Cromite's "Partitioning all cookies" patch is undone (patch 0008).** It deleted every unpartitioned
cookie each time the browser started, so logins taken from the shared folder lasted until the next
start, and the merge then took their absence for a logout on every machine. It also stopped an extension's
requests carrying a site's cookies. Cookies now behave as in Chrome, and third-party ones can be blocked in
Settings.

**Nothing lost here is lost everywhere.** A store that comes up without most of what was agreed (the browser
killed before it wrote them, a key it cannot read them with) is told apart from a wearer logging out: that
happens during a session, one site at a time, or by clearing browsing data, which is recorded. The cookies
come back from the folder instead of being removed from it. A profile with no folder at all changes nothing.

**In the room**, Chromium draws every bubble -- the profile menu, the address bar's suggestions, a login
field's list, the save question, permission questions -- as a Wayland subsurface. The room now draws those
and points at them as it does a window's menus (`push_subsurfaces` in `scene.rs`); before, they opened unseen.

Checked in the Deck's test room against a copy of the shared folder (2026-10-30): the four profiles with their
names, colours and order; Cec's lock; a bookmark, a visit and a cookie each way, while open and while closed, and
the WPE build reading what Chromium wrote; 79 cookies through two restarts with the folder unchanged; a profile
removed elsewhere; a folder extension and a store extension arriving on a browser that had neither.

**Locks from the profile menu (patch 0012).** "Set a PIN or password" (or "Change its PIN or password") opens a window
of its own: nothing, a PIN or a password, typed twice, changed or removed only after the present one, written to the
shared `profile.json` so WPE and every machine agree. "Lock profiles now", there once any profile has a lock, puts every
locked profile's windows out of sight and hushes their tabs until it is given; "Close its windows" is the way out, so
no profile is left hidden with nothing asking for it. The lock window also offers "Open with a fingerprint" where
SpatiKey is usable: `profile_unlock`, always a new touch, even with a touch set to last.
Settings' "You and Google" is now "Your profile": no sign-in, sync or Google rows, only Customize profile and Import,
which have no other home. A page of a locked profile may run hidden behind the lock
until the PIN is given, as the window is what waits.

Settings has no Default browser page: the room decides what opens links. Adblock sits under Privacy and
security, where it shows without scrolling the menu. Cromite's firewall (`services/firewall/tools/rules.xml`)
lets only listed downloads out; the spell-check dictionary is listed now, so it fetches from Google's
`redirector.gvt1.com` once per language (patch 0015).

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

**WPE (every platform now).** A page's own process crashing takes only its tab, as above. The
engine host is WebKit's UI-process half and SpatiMesh's own C beside it (`native/wpe.c`), so it
has to be as sturdy as a browser's main process: what a page does must never reach it as anything
but events. On 2026-10-29 it did -- a window an advertisement opened (`window.open`), whose tab
was closed at once, kept running with a reference the engine leaked, and its next load told the
freed tab what it did; the engine read freed memory and stopped, with all ten tabs of the
profile. Now a tab owns its view's only reference (one `window.open` made but never taken is
ended after a minute), and a tab's handlers are disconnected when it goes, whatever still holds
its view. Engine tests `a_window_opened_and_closed_runs_no_more` and
`a_closed_tab_still_answering_says_nothing_more` run with freed memory scribbled over, so a
regression fails at once.

**An engine host that stops anyway is started again at once** (`engine_gone`,
`restart_engine`): each window's tab in front is loaded again, the others when they are next
shown, as when a profile's windows open again, and a toast says what happened. (A tab that was
playing would play nothing loaded behind: WebKit plays no sound before a click.) Its developer
tools close. An engine that stops three times in two
minutes is left stopped, its tabs saying so until one is loaded again: a page that stops it
whenever it loads cannot keep it stopping.

**A tab behind another is hidden to its page** (`Engine::show`, `ToHost::Show`): WPE unmaps its
view, so the page is `document.hidden`, WebKit draws it no more and slows its timers, as every
browser does a background tab -- ten open tabs cost what the ones in front do. Its sound plays on.
Extensions' pages no window has as a tab are never hidden.

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

**Fixed in Spatiand's WebKit** (`third_party/wpe/patches/webkit/0002-visited-links-per-context.patch`):
one `VisitedLinkStore` per `WebKitWebContext`, set on every page configuration the context makes,
so every tab of a profile shares one -- WebKit adds to it as any of them navigates, and tells
every web process using it -- with two calls of its own: `webkit_web_context_add_visited_links`
(addresses visited elsewhere: other machines' history, and before SpatiMesh last started) and
`webkit_web_context_clear_visited_links` (a history cleared). The engines use them where the
header says they are there (`WEBKIT_WEB_CONTEXT_HAS_VISITED_LINKS`); the window tells them
(`ToHost::Visited`) and, when pages are taken out of the history here or on another machine,
has them forget every visited link and tells them again what is left (`ToHost::ForgetVisited`).
`spatimesh-wpe`'s `a_link_visited_in_another_tab_looks_visited` and
`visited_links_are_told_and_forgotten` are the tests; both pass on Linux against WPE built with
the patches (2026-10-27). The distro's WebKitGTK has not got the
patch, and keeps its links per tab.

## WPE WebKit, SpatiMesh's own engine (2026-10-22 to 2026-11-10)

*Removed on 2026-11-10 (see the top): the record of the port, kept for what it found.*

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
| WebGL | **Every frame already reaches the window as an IOSurface** (the compositor's, with WebGL's canvas in it), nothing copied; what WebGL itself draws is a texture of the ANGLE the compositor draws with, shared with it, also not copied. What is in between is WebKit's own ANGLE running on its GL back end over that other ANGLE: every call and every shader translated twice. `patches/webkit/0004` puts WebGL on WebKit's ANGLE's own Metal back end instead, drawing onto IOSurfaces (three, in turn) that the compositor's ANGLE takes as textures of the same memory, with a fence the compositor waits on (`GraphicsContextGLTextureMapperIOSurface`; `WEBKIT_WEBGL_DISABLE_IOSURFACE=1` goes back to the old way, as does anything missing). **Written, not yet compiled**: it is Mac-only code and there is no Mac here. |
| Fonts | fontconfig with Spatiand's own `fonts.conf`: each script's macOS font by language; WOFF2 web fonts (Google's woff2, on brotli); the interface font reported by SpatiMesh's display (WPE had none for form controls: patched). |
| Developer tools | WebKit's Web Inspector, as a tab after its page (`ToHost::Inspect`; the menu's "Developer tools", Ctrl-Alt-I or Command-Option-I). It closes with its page. Not on WebKitGTK or WKWebView, whose inspector is a window of its own. |
| Files | `file://` pages load (GIO's type identifiers taken as MIME types, patched). |
| Extensions | The shared end-to-end test passes. |
| Linux | `third_party/wpe/build.sh linux`: a bundle for SteamOS, built in the Ubuntu 22.04 root and carrying every library but glibc, libstdc++, Mesa, OpenSSL 3 and a few every desktop has, found by `$ORIGIN` (`patches/webkit/0003` makes WebKit's own paths relative too); `SYSTEM_DEPS=1` builds it on the distro's libraries, for working on it: built so on Ubuntu 24.04
with all six patches, and there `spatimesh-wpe`'s 21 engine tests pass (four runs in a row), its
extensions test too, and SpatiMesh's own end-to-end run in the compositor (`SPATIMESH_E2E_ENGINE=wpe
tools/spatimesh-ui-e2e.py`, every check). The release carries it in `wpe/` beside `spatimesh-wpe`, which SpatiMesh then prefers to WebKitGTK. Pictures are copied (shared memory). |
| Android | `third_party/wpe/build.sh android`: an NDK cross build; every library an unversioned `lib*.so`, WebKit's processes as `lib<process>.so` (found there by `patches/webkit/0003`), all in the APK's library folder (`android/build.sh`). The engine reads WPE's AHardwareBuffer pictures back (by the CPU, or through GL). **Written, not yet built**: there is no NDK here. |
| Networking | Already as parallel as a browser's: each profile's engine has its own network process (one profile's stalled requests cannot hold up another's), whose libsoup keeps up to 256 connections, 6 to a server, with HTTP/2 multiplexing where the server speaks it; TLS runs on a thread of each connection's (glib-networking), and the disk cache on its own threads. What SpatiMesh adds is not waiting: a link pointed at, and where the address bar would go as it is typed, are connected to before the click -- name, TCP and TLS (`ToHost::Preconnect`, `patches/webkit/0005`'s `webkit_web_view_preconnect`; a name looked up only, where the engine has not got it). |
| Not yet | Video frames as IOSurfaces into ANGLE; pictures handed over without copying on Linux (DMA-BUF) and Android (AHardwareBuffer); the developer tools on Android (their pages are not in the APK); the developer tools' text size in the glasses; the interface font from each platform's own. |

The patches (`third_party/wpe/patches/webkit/`), applied in order to a checkout that has not had
them, which `build.sh` writes down in it (`.spatiand-patches`): 0001 the Mac port; 0002 visited
links shared by a context's views; 0003 relocatable on Linux and Android; 0004 WebGL on Metal onto
IOSurfaces (Mac); 0005 preconnecting; 0006 a function of 0001's out of the header it was in, which
not every file including it could compile (Linux's first build found it).

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
