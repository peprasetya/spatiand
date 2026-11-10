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
- On the Mac that holds in the room **and on the Mac's desktop** (2D mode): Spatiand shows its
  clients in Mac windows. An app never draws its own native window instead.
- **SpatiMesh is the Deck's Chromium** -- Chromium's Linux UI, Ozone/Wayland -- everywhere. Never
  Chromium's Cocoa (Mac) browser, nor Android's Chrome, nor any platform's own browser shell. A
  Cocoa build of Chromium was made on 2026-11-10 against this; it is not SpatiMesh and must not be
  shipped, offered as a stopgap, or built on (docs/spatimesh-mac-port.md is the way).
- A port follows the Deck's build: the same code and the same protocol, carried to the other
  system. Nothing borrowed from another build's output as a shortcut, and no platform-native
  substitute. Where a workaround is unavoidable, it is named at once with the proper alternative.
