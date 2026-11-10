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
