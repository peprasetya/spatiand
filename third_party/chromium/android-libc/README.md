# Chromium's Linux browser on Android's libc

SpatiMesh on Android is the Deck's: Chromium's Linux browser, with Cromite's patches and ours
(`../patches/`), built for Android's libc and carried in Spatiand's APK beside its other programs,
a Wayland window of the room like them (docs/spatimesh.md, "SpatiMesh for Android"). These are
the changes that build needs and the Deck's does not, applied after ours, with `patch -p1 -F 3`
(some reach into dependencies that are repositories of their own), and only for
`CHROMIUM_TARGET=android`.

**`termux/`** is termux-packages' `x11-packages/chromium-host-tools/cr-patches` at commit 47d0a63
(2026-11-08), made for Chromium 157, as they are (since SpatiMesh itself is on 157, 2026-11-14, `7007` too: 157's `ValueCounter` needs it): Termux builds the same thing, Chromium's Linux
code with `--target=aarch64-linux-android24`. Their macro, `__TERMUX__`, is kept, so that a newer set
can be taken without editing it; here it means built for Android's libc. Licensed as Chromium is
(termux-packages' LICENSE.md: a package's patches carry the package's licence).

Not taken, and why:

| Theirs | Why not |
|---|---|
| 0007 compiler: no ELF CREL | 153's line differs: ours is `0001-no-elf-crel.patch` |
| 0008 tools: no CFI | 153's library loader has no `cfi_unchecked_callee` yet |
| 1019 a dummy HID service | 157 has an Android branch there and 153 none, so with fuzz it went in the wrong place: ours is `0008-dummy-hid-service.patch` |
| 1009 no Google API warnings | Cromite already takes it out |
| 1022, 2026 HEVC in the bundled FFmpeg | SpatiMesh decodes with Spatiand's FFmpeg (patch 0003) |
| 2020 dav1d's configuration | dav1d is not built (the owner's choice: AV1 on the GPU only) |
| 2025 OpenXR without `secure_getenv` | Cromite builds no OpenXR |

Of those taken, 48 apply to 153 as they are and 22 with fuzz. **Worth knowing about two of them:**
1001/1004 turn Chromium's sandbox off (Android lets no program make namespaces; a sandbox of
seccomp alone is still to do), and 1005 gives Chromium no DNS settings of the system's -- names are
then looked up by Android's own resolver -- and, where nameservers are asked for, Google's 8.8.8.8.

**`deps.sh`** builds what Chromium's Linux code takes from the system and Android's libc has not
got, as termux-packages builds them: NSPR 4.40 and NSS 3.133 (shared), libdbus 1.16.2 (static),
and libffi 3.4.8 (Wayland's) and SQLite 3.54.1 (NSS's database), shared as libffi.so and
libsqlite3.so, the names an APK carries. xkbcommon is the compositor's (android/xkbcommon). `deps-patches/` are Termux's for them (packages/libnspr,
libnss). With them the Deck's code for certificates and D-Bus is built as it is.

**Ours**, after Termux's:

| | |
|---|---|
| 0001 | No ELF CREL relocations (Termux's 0007, on 153's line). |
| 0002 | Wayland's clipboard without D-Bus (no longer needed now D-Bus is built, and harmless): there is no portal, and files go between windows by their paths alone. |
| 0003 | `__TERMUX__` defined for what runs on Android (`spatiand_android_libc = true`), not for the programs the build itself runs: Termux's own build defines it, Chromium's does not. |
| 0004 | Chromium's own fontconfig without gettext, glibc's `random_r` or libuuid (it only reads `.uuid` files), and its own expat, which a Linux build takes from the system: Android's libc has neither (Termux has both in its prefix). |
| 0005 | SwiftShader without Wayland surfaces of its own: it draws only off screen here, and a Linux build finds Wayland's headers in the system's. |
| 0006 | V8's stack traces without `backtrace()`: Android's libc has it from API 33, and this builds for 24, as Termux's changes are written for (Termux brings libandroid-execinfo). |
| 0007 | minigbm without Android's system properties (`cutils/`, the platform's, not the NDK's), as Termux's 2013 does for its log. |
| 0008 | A dummy HID service, Termux's 1019 placed for 153: no HID devices without udev. |
| 0009 | The Linux notifications bridge depends on `//components/dbus`, whose headers it includes: without GTK and GLib nothing else brings their path to it. |
| 0010 | Its resources (`.pak`, ICU's data, V8's snapshot, the locales) from `SPATIMESH_ASSETS` when set: in the APK the program is in the library folder, which holds `lib*.so` alone, and the app unpacks the rest into its files (`run-android.sh` sets it). |
| 0011 | libc++ linked in whole (`-static-libstdc++`), for Android's processor only: the NDK's Chromium builds with is newer than the r28c one the APK carries for Spatiand's other programs (`std::__hash_memory`); the build's own programs link the build machine's libstdc++, which has no static one. |
| 0012 | Sound and the microphone through Spatiand's room (`media/audio/spatiand`): each stream to `$XDG_RUNTIME_DIR/spatiand-sound` under its tab's window's sink name, the microphone from `spatiand-mic`. Android has no PulseAudio to play into the room's sinks as the Deck does. |
| 0013 | Chromium's seccomp sandbox without namespaces: Termux's 1001 forces `--no-sandbox`, taken out for `chrome`; the zygote starts without a first layer, which Android lets no app have; bionic's ways allowed as for Chrome on Android (not its baseline, which leaves files to SELinux); the main thread's stack asked before the sandbox (`base/spatiand_main_thread_stack.cc`), as bionic reads `/proc/self/stat` for it on every call. |
| 0014 | The GPU draws off screen and each frame goes to the room as shared memory (Ozone's readback surface, which upstream uses for software GL): Android's EGL and Vulkan make no Wayland surfaces. Since 0021 only where the room takes no hardware buffers. With `angle_use_custom_libvulkan = false` (build.sh) ANGLE takes Android's Vulkan loader, which reaches the GPU's driver. |
| 0015 | Video decoded by Android's hardware decoders: the GPU process's media client is Spatiand's (`media/mojo/services/spatiand_media_codec_video_decoder.cc`, MediaCodec through the NDK) in place of the stub a Linux build without VA-API has; pictures copied into frames of memory; "video_decoder" in `mojo_media_services` and `AcceleratedVideoDecoder` on by default, as with VA-API. |
| 0016 | The GPU process sandboxed: the filter put on every thread at once (TSYNC), as Android's GPU drivers have started threads by then; the broker given the kgsl device, binder, the vendor's and system's libraries and settings, the driver's shader cache and sysfs; binder opened before the sandbox; what SELinux hides among open files not taken for a directory; mremap, msync, getcpu, membarrier, rt_sigtimedwait and set_tid_address allowed, as on Chrome for Android. |
| 0017 | Ten-bit video (HEVC Main 10, VP9 profile 2) on the hardware decoders too, asked for as P010, with the stream's colour space and HDR metadata. |
| 0018 | The crash handler as `libspatimesh_crashpad.so`, in the APK's library folder beside the browser (`build.sh` package, `android/build.sh`): crashes leave minidumps in `Crash Reports` as on the Deck. |
| 0019 | Files chosen in Spatiand's own chooser (`crates/spatiand-choose`, `libspatiand-choose.so`), run as a program, where on the Deck the desktop portal is asked: Android has none. |
| 0020 | Frames read back for the room as RGBA, into a pixel buffer kept from frame to frame, red and blue swapped by libyuv: asked for BGRA, ANGLE on Vulkan converted one pixel at a time (190 ms a frame for a large window, five frames a second). Only where the room takes no hardware buffers (0021). |
| 0021 | Frames to the room in Android hardware buffers: each copied on the GPU into one, the buffer sent once through a socket that stands as a dmabuf's file under Spatiand's own modifier (`0xf500000000000001`), which the room's EGL turns back into the buffer (`android/eglshim`). ANGLE takes such buffers as EGLImages (`third_party/angle`, `linux/AHardwareBufferImageSiblingVkSpatiand`; the patch carries ANGLE's part, a repository of its own, after Chromium's); the allocator's mapper is loaded before the GPU sandbox; a frame goes once its fence has signalled (at once with its fence since 0024); frames in flight keep their places through a resize. |
| 0022 | The GPU process asks its own path before the sandbox: Dawn's information, two minutes after it starts, asked it of `/proc/self/exe`, which the broker does not read, and the process ended. |
| 0023 | No D-Bus: connecting to the system or session bus fails at once, and calls without a bus are not logged (libdbus looked for a bus under the path it was built in). |
| 0024 | Frames sent to the room at once, each frame's fence (`EGL_ANDROID_native_fence_sync`) sent just before down the socket its buffer came through, which the room keeps and its GPU waits on (`android/eglshim`, `explicit_sync.rs`): looked at on the GPU main thread, the fence waited behind tile raster. Chromium's Wayland host knows only `wp_linux_drm_syncobj_v1` for fences, which needs DRM. |
| 0025 | Bluetooth through the Spatiand app: the adapter on Android's libc is Android's Bluetooth LE, reached through `android/src/id/prasetya/spatiand/BluetoothBridge.java` (an abstract socket, `spatiand-bluetooth`, one JSON object a line), where Linux's is BlueZ on D-Bus (`device/bluetooth/spatiand`). Scanning, GATT connections, services, characteristics, descriptors, notifications: what Web Bluetooth asks for. |
| 0026 | What Chromium 157 changed under the patches above: `gfx::FrameData` is moved, not copied (the frames to the room, 0021 and 0024), and ANGLE's `DeriveCreateInfoPNext` takes the intended format before the actual one (0021's sibling). |
