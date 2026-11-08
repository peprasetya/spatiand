# Chromium's Linux browser on Android's libc

SpatiMesh on Android is the Deck's: Chromium's Linux browser, with Cromite's patches and ours
(`../patches/`), built for Android's libc and carried in Spatiand's APK beside its other programs,
a Wayland window of the room like them (docs/spatimesh.md, "SpatiMesh for Android"). These are
the changes that build needs and the Deck's does not, applied after ours, with `patch -p1 -F 3`
(some reach into dependencies that are repositories of their own), and only for
`CHROMIUM_TARGET=android`.

**`termux/`** is termux-packages' `x11-packages/chromium-host-tools/cr-patches` at commit 47d0a63
(2026-11-08), made for Chromium 157, as they are: Termux builds the same thing, Chromium's Linux
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
| 7007 extensions' `ValueCounter` | 153 has no `distinct_count` yet |

Of those taken, 48 apply to 153 as they are and 22 with fuzz. **Worth knowing about two of them:**
1001/1004 turn Chromium's sandbox off (Android lets no program make namespaces; a sandbox of
seccomp alone is still to do), and 1005 gives Chromium no DNS settings of the system's -- names are
then looked up by Android's own resolver -- and, where nameservers are asked for, Google's 8.8.8.8.

**`deps.sh`** builds what Chromium's Linux code takes from the system and Android's libc has not
got, as termux-packages builds them: NSPR 4.40 and NSS 3.133 (shared, against the WPE build's
SQLite), and libdbus 1.16.2 (static). `deps-patches/` are Termux's for them (packages/libnspr,
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
