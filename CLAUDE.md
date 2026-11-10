# Spatiand: rules for every session

## Every platform is treated the same

The owner, 2026-11-10: **"all platform shall treated the same... android and mac! same code base for
core... only need to add platform related!!!"**

- One core codebase for the Deck, Linux, Android (Beam Pro) and the Mac. A port adds only what the
  platform itself needs (its libc, its windowing to show the room, its GPU buffers, its sound); it
  never forks, rewrites or substitutes the core, and never picks a platform-native alternative.
- Before building anything for a platform, check that the plan is the Deck's code plus a platform
  layer. Long builds are planned so that one build is the right one.

## Compiling on the owner's machines

- littleotter (the Mac mini) compiles only when the owner says so, normally at night; never a long
  build while the owner is using it. deepmagpie compiles any time.

## Every app built in this project is a Spatiand app

The owner, 2026-11-10: **"All app build in this spatiand project is spatiand app."**

- Every application this project builds -- SpatiMesh, SpatiQuill, SpatiShelf, SpatiReel,
  SpatiLoupe, SpatiPal, SpatiKey and any to come -- is a **client of Spatiand, speaking Wayland to
  it**, on every platform: the Deck, other Linux, the Beam Pro and the Mac.
- **They follow the notes of `spatiand_xr_v1`**, the room's own protocol
  (`crates/spatiand-proto/protocol/spatiand-xr-v1.xml`, its descriptions are the notes): the room's
  extensions on top of Wayland -- eye layouts, layers, idle fade, resize anchor, see-through, sound
  key, pinch, scroll, `describe` for dictation and the assistant, the pose channel -- used as they
  say, the same on every platform. An app does what that protocol offers through it, not by a
  platform's own means; what it lacks is added to the protocol (and to every client's copy of it,
  e.g. Chromium's in `third_party/wayland-protocols/spatiand`), not worked around in one app.
- On the Mac that holds in the room **and on the Mac's desktop** (2D mode): Spatiand shows its
  clients in Mac windows. An app never draws its own native window instead.
- **SpatiMesh is the Deck's Chromium** -- Chromium's Linux UI, Ozone/Wayland -- everywhere. Never
  Chromium's Cocoa (Mac) browser, nor Android's Chrome, nor any platform's own browser shell. A
  Cocoa build of Chromium was made on 2026-11-10 against this; it is not SpatiMesh and must not be
  shipped, offered as a stopgap, or built on (docs/spatimesh-mac-port.md is the way).
- A port follows the Deck's build: the same code and the same protocol, carried to the other
  system. Nothing borrowed from another build's output as a shortcut, and no platform-native
  substitute. Where a workaround is unavoidable, it is named at once with the proper alternative.

## How each app speaks Wayland (the same on every platform)

Every app is a Wayland client of Spatiand; what differs is only the language its Wayland code is in,
and on every platform that code is built into the program -- nothing is installed on the machine:

| App | Wayland client code | Language |
|---|---|---|
| SpatiReel, SpatiLoupe, SpatiQuill, SpatiShelf, SpatiPal, SpatiKey, ... | `wayland-backend`'s own implementation (`src/rs`, the `client_system` feature off), through Spatiand's SDK `crates/spatiand-app` | Rust |
| SpatiMesh (Chromium) | Chromium's own libwayland (`third_party/wayland`, `use_system_libwayland = false`), compiled into the program | C |

Checked 2026-11-10: on the Deck `spatireel` links no libwayland, and `spatimesh` links none from the
system but carries Chromium's (`wl_display_connect_to_fd` inside it); the Beam Pro's
`libspatimesh_chromium.so` likewise. The Mac's SpatiMesh compiles the same `third_party/wayland` (its
checkout lacked the sources only because Chromium's DEPS marks them Linux-only; copied from the Deck's
tree of the same version). Chromium cannot use the Rust crate: its Ozone/Wayland is C++ against
libwayland's C API. Both speak the same protocol, `spatiand_xr_v1` included, on the same socket.
