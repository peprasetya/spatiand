# SpatiMesh, Spatiand's web browser

A full browser in the class of Safari and Chrome: the Deck's Chromium (its Linux UI, Ozone/Wayland) with
Spatiand's patches, a Wayland client of the room that speaks `spatiand_xr_v1`, the same program on the Mac, the
Deck, other Linux and the Beam Pro. Asked for on 2026-10-16.

| Platform | Engine | Where it comes from |
|---|---|---|
| Linux, the Deck | **Chromium** with Spatiand's patches, Cromite's among them (`third_party/chromium`, "The monthly update" below) | `~/spatiand-chromium`, packaged on its own |
| Android (Beam Pro) | **Chromium, the Deck's**, built for Android's libc (2026-11-08; "SpatiMesh for Android" below) | in the APK, `libspatimesh_chromium.so` and its resources |
| Mac | **Chromium, the Deck's** (Linux UI, Ozone/Wayland, carried to macOS: docs/spatimesh-mac-port.md); in the room and on the Mac's desktop as a client of Spatiand. Hardware video decoding on Chromium 157 checked 2026-11-18 | `CHROMIUM_TARGET=mac`, built on littleotter |

History: until 2026-11-10 SpatiMesh was a Rust window program (`crates/spatimesh`) over WPE WebKit and
GStreamer (before that WKWebView, Steam's CEF, WebKitGTK and Android's WebView). All of it is removed; Chromium
alone is SpatiMesh on every platform. What the Rust window alone still did, Chromium's patches do since that day,
each checked:

| The Rust window's | Chromium's | Checked |
|---|---|---|
| Profiles, locks, bookmarks, history, logins, settings, extensions on every machine | patches 0008-0016 (below) | the Deck's test room, 2026-10-30 |
| The `home` setting | patch 0022: Chrome's home page, both ways ("about:blank" is the new tab page) | headless: Settings to `home.json` and back |
| Extensions' `storage.sync` in the shared folder (`extension-sync/<ext>/<key in hex>.json`) | patch 0021, `chrome/browser/spatiand/storage_sync.cc` through StorageFrontend | headless: set, then changed and removed "elsewhere", `onChanged` heard all three |
| Answering the room's `describe` for dictation and the assistant | patch 0020: `spatiand_xr_v1` per toplevel (Ozone) and `chrome/browser/spatiand/describe.cc`, the same script and answer | snapshot room (`describe` step): field last, no password |
| Films in its own player: every container and codec of Spatiand's FFmpeg | patch 0019: AVI, MPEG-TS/PS, FLV, ASF, AIFF, CAF, bare AC-3/E-AC-3/DTS let through; MPEG-4 Part 2, MPEG-2, VC-1, DTS, ALAC decoded; MP3 plays again (it had not since 0003) | headless on deepmagpie, the Deck and the Beam Pro |
| A page dropped from SpatiShelf on its tabs | Chrome's own | `tools/spatiand-dnd-e2e.py` with `SPATIAND_CHROMIUM_BIN` |

Not yet in Chromium (the Rust player had them): H.264 and HEVC in an MPEG transport stream (no frame decoded; with
StrictFFmpegCodecs a decode error at the first frame), an MPEG program stream (VOB: "PTS is not
defined"), MP2 sound, and a film's own subtitle tracks. Firefox add-ons (`.xpi`), which the Rust
window's own extension runtime took, Chrome does not (the owner: not wanted). On the Mac the Deck's Chromium runs in littleotter's snapshot room
(docs/spatimesh-mac-port.md); `tools/spatiand-dnd-e2e.py` drops onto Chromium only where one is
named. `crates/spatiand-media`'s MediaSource (`mse.rs`, `Player::open_source`) was the Rust window's alone and was
removed on 2026-11-17, with what else only its pages used there (`hardware.rs`'s list of codecs for pages, a sound group's hush and change of name).
The Beam Pro's Chromium had been built without Dolby (`enable_platform_ac3_eac3_audio`, the Deck's);
it is built with it since. 
**Chromium, from 2026-10-30:** the owner moved SpatiMesh to Chromium with Cromite's patches and ours (Cromite's
no more: the monthly update below), keeping Chrome's own browser layer (Chrome's extensions, as Chrome has them) with
SpatiMesh's look over it, as Vivaldi does. Ours to add: drawing through Spatiand's own Wayland protocol on every
platform (Chromium's Linux drawing, Ozone/Wayland, taken to the Mac and Android), media on Spatiand's FFmpeg,
SpatiKey's passwords and passkeys, profiles synced through spatiand-host. See "Chromium on Spatiand's data" below.

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
* **The pictures stay on the GPU** (2026-11-17, `android-libc/0028`, with the sandbox's part in `0016`): 1080p
  at 60 frames a second was too much for the copies above (HEVC played at 45 frames a second, 150 dropped). Now
  the decoder draws into an `AImageReader`'s Android hardware buffers (`AIMAGE_FORMAT_PRIVATE`, YUV in the GPU's
  own layout); each picture becomes a shared image (`SpatiandHardwareBufferImageBacking`: an external texture
  bound to an EGLImage of the buffer, the GPU waiting on the decoder's fence) that the compositor samples as it is,
  and the buffer goes back to the decoder once the page lets its frame go, as the Deck's VA-API pictures stay on its
  GPU. ANGLE takes such a buffer through a Y'CbCr conversion and the external format Vulkan knows it by, as its
  Android build does (`AHardwareBufferImageSiblingVkSpatiand`). Two things the numbers turned on: pictures are drawn
  into the reader one at a time (one drawn while the one before was still in the reader's queue replaced it, and
  60-frame H.264 reached the page at 42 frames a second), and the reader holds 24, the renderer's own cap (with ten,
  60 frames a second played at 36). Where a reader cannot be had (no GPU channel, a codec that will not draw into
  one) the pictures are copied out as before. Found the same day: in the sandbox every decoder failed to start
  (NO_MEMORY), so films were decoded in software: the codec service takes MediaCodec's input in Android's shared
  memory, which Android's libc opens as `/dev/ashmem<boot_id>` after reading the boot id, both now given to the
  broker. Measured on the Beam Pro: 1080p60 HEVC at 59 frames a second, 2 to 9 dropped. Not measured: ten-bit pictures (P010) through the reader, 4K, and anything in the glasses.
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
  Chromium), noted beside the pinned versions in `third_party/chromium/build.sh`. Tried on the Beam
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
  path a line (`--mode open|open-multiple|folder|save`, `--dir`, `--name`, `--accept`). It is SpatiMesh's picker
  as a program: places, the folder's lines (the cursor starts on them, and finding moves it to the first found, so
  Enter takes it), a field to find in or type a path into, several marked then chosen, a name to save under (asked
  before writing over a file). Tried in the Mac's snapshot room by script (a file found and taken, a page saved, two
  marked and chosen) and on the Beam Pro from SpatiMesh: its window opened in the room, a choice reached the page,
  closing it was a cancel. Android's own shared storage (Download, Pictures) is offered, and downloads go to its
  Download folder (`run-android.sh`), only once Spatiand has "All files access", which the app asks for on Android's
  own page (`MainActivity.askForAccess`; android/setup.sh grants it from a computer).
* **Crashes leave minidumps** (`android-libc/0018`): Chromium's crash handler is carried as `libspatimesh_crashpad.so`;
  a tab crashed on purpose (`chrome://crash`) left its `.dmp` in `$XDG_CONFIG_HOME/chromium/Crash Reports/pending`.

## Chromium on Spatiand's data (2026-10-30)

Chromium keeps its own menus, profile menu, bubbles and settings; what they show and change is what
every Spatiand machine shares. All of it is `chrome/browser/spatiand/` (patch 0009) with a few lines of
hooks, reading and writing the shared folder's files, so every machine and spatiand-host agree:

| What | Where it is shared | Chromium's side |
|---|---|---|
| Profiles: name, colour, order, lock | `apps/spatimesh/profiles/<id>/profile.json` | `profiles_sync`: `default` is `Default`, another is `Spatiand-<id>`; listed without being opened until they are used; added and removed both ways |
| A profile's lock | the same file (salted hash) | `profile_lock_dialog`: its own window, PIN or password, before any of that profile's windows shows; again after its last window closes |
| Bookmarks | `profiles/<id>/bookmarks/<id>.json` | `bookmark_sync`: the bookmark bar is the top level, Other bookmarks the folder `other`; fractional order keys as spatiand-host's side makes them |
| History | `profiles/<id>/history/<machine>-<YYYY-MM>.json`, `forgotten-<machine>.json` | `history_sync`: this browser's own machine id; last 90 days taken in as real visits |
| Logins | `profiles/<id>/cookies/<site>.json` | `cookie_sync`: a three-way merge, site by site, of the browser's cookies against the files and what the two last agreed on |
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
pages name are the generic ones, type hinted lightly). The black was of two kinds. The band
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
names, colours and order; Cec's lock; a bookmark, a visit and a cookie each way, while open and while closed; 79 cookies through two restarts with the folder unchanged; a profile
removed elsewhere; a folder extension and a store extension arriving on a browser that had neither.

**Locks from the profile menu (patch 0012).** "Set a PIN or password" (or "Change its PIN or password") opens a window
of its own: nothing, a PIN or a password, typed twice, changed or removed only after the present one, written to the
shared `profile.json` so every machine agrees. "Lock profiles now", there once any profile has a lock, puts every
locked profile's windows out of sight and hushes their tabs until it is given; "Close its windows" is the way out, so
no profile is left hidden with nothing asking for it. The lock window also offers "Open with a fingerprint" where
SpatiKey is usable: `profile_unlock`, always a new touch, even with a touch set to last.

**History is one source of truth (patch 0030; the owner, 2026-11-17).** Each machine's history converges to the shared folder: every visit file, its own included, is read when its size, time and SHA-256 say it changed (`local/.../history-seen.json`; the first start of this build reads them all), and what history lacks is added, matched by address and millisecond, doubles of imported visits taken out. A clearing no person made (bounce tracking's, from the start of time) writes no forget-record and history is refilled from the folder; a person's "All time" is written as from 0 and clears every machine; removing single visits from the history page is written per address. Restored visits come back as plain link visits.

**A start never opens a locked profile (patch 0012; the owner, 2026-11-14).** A profile with a lock is not recorded
as last used or as open at exit, also when it was unlocked in that run (`profile_manager.cc`), and what was recorded
before, or locked since on another machine, is forgotten at the start (`ProfilesSync::ForgetLockedAsLastUsed`): a
plain start opens the unlocked profile used last. A start that names a locked profile (`--profile-directory`) opens
that one too, not the named one's PIN window; a locked profile is reached only from the profile menu. With every
profile locked the start makes a new, unlocked profile, as a first start does, and it is shared like any other. Not
changed: a command line handed to a browser that is already running and names a locked profile shows its PIN
window (nothing of the room's does that); and a lock set on the profile in use keeps it the one links open in for
the rest of that run.
Settings' "You and Google" is now "Your profile": no sign-in, sync or Google rows, only Customize profile and Import,
which have no other home. A page of a locked profile may run hidden behind the lock
until the PIN is given, as the window is what waits.

Settings has no Default browser page: the room decides what opens links. Adblock sits under Privacy and
security, where it shows without scrolling the menu. Cromite's firewall (`services/firewall/tools/rules.xml`)
lets only listed downloads out; the spell-check dictionary is listed now, so it fetches from Google's
`redirector.gvt1.com` once per language (patch 0015).

## The monthly update (from 2026-11-14)

**SpatiMesh is on Chromium 157.0.8103.78 on all three systems, and its patches are Spatiand's own**
(the owner, 2026-11-13: Cromite had stopped at 153 on 2026-09-15, and he would not wait for it;
meanwhile 157 mended a hole in use against people, CVE-2026-125350). What was taken over from
Cromite, what was left, and how the patches are kept: `third_party/chromium/patches/README.md`.
Every month the version moves to the newest stable Chromium, the same one on the Deck, the Beam Pro
and the Mac.

**Which version:** Linux's stable, on the day (chromiumdash.appspot.com, channel Stable). Termux's
changes for Android's libc (`android-libc/termux`) must be for the same major version: if Termux is
a version behind, the update waits for it or carries those 70 itself.

**The steps**, each on all three before the next is called done:

1. *The tree.* `gclient` syncs the trees that are there to the new tag (the patches are commits in
   the tree and in four of its dependencies; set those aside, sync, put them back). A fresh fetch
   is `build.sh <work> fetch`: about an hour and 30 GB a tree. The Mac's checkout leaves out the
   Linux-only sources (libwayland, wayland-protocols, libdrm, minigbm, libsync, lss, fontconfig,
   cros_system_api): copied from the Linux tree of the same version.
2. *The patches carried*, in a worktree (`third_party/chromium/rebase/`): `drive.sh`, then the
   conflicts by hand, every one written down; then **`audit.py`**: no patch may have lost a file
   without a line that says why. The Mac's port (`mac/wip`) the same way with `applyeach.py`, and
   `mac/gn-ui.py` run again over the new build files. Android's (`android-libc`) with `patch`, in
   order.
3. *Built* on Linux first, past every error (`-k 0`), each fix to the patch's intent and written
   down; then Android and the Mac with Linux's fixes already in. A fix in shared code goes to all
   three trees.
4. *Compared with the version in use*, in each system's test room, never on the owner's data: the
   Deck's (`tools/chromium-test`), the Mac's (`tools/spatimesh-mac-test`), the Beam Pro's without
   an install (its notes in `android-libc/README.md`). At least: starting with a clean log, pages,
   every codec, the privacy values, ad blocking, every Settings page (they are where carried hunks
   go quietly wrong), the shared folder's profiles and bookmarks, passwords against the stand-in
   service. A failed check does not stop an official build: read the log for `Check failed`.
5. *Written back:* each fix given to the patch that owns its file (`attribute.py`, autosquash),
   the patch files made again from the tree, and **applied to a clean checkout to see that they
   give the same tree**. `CHROMIUM_VERSION` in `build.sh`; this page.
6. *Installed* on the three, the version before kept beside it until the owner has used the new
   one.

**The first carry, 153 to 157 (2026-11-13/14):** 310 patches carried, 305 files written (five had
only been changes in dependencies or were mended upstream). Found on the way and mended: the
settings sync set the default search engine from inside the search service's own notification,
which 157 checks at every start; **every Settings page on the Mac ended its tab**, on 153 as well,
for two strings Cromite's "clear data on exit" defines for Windows and Linux only and its page
names everywhere (very likely the owner's Settings fault, which the Deck never showed, since there
they exist); the build's generated flag lists, stale after a patch added to them. In the test rooms
157 gave what 153 gives on the Deck, the Mac and (headless) the Beam Pro. Not seen by anyone in the
glasses yet. Left as they are, for the owner: the ad blocker's lists still come from Cromite's
pages; Chromium 157 reads a kind of HDR metadata track (ITU-T T.35) through an FFmpeg newer than
Spatiand's 9.0.2, so that track is skipped; 157's on-device model switch is one with its
server-side one, so Cromite's line that turned the first off is gone with the feature it named;
HEVC goes to VA-API on the Deck on both versions, which is as the owner wants it (video decoded by the hardware, whichever way; `run.sh` said otherwise and was corrected).
