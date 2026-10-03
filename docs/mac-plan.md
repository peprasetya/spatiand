# Spatiand on macOS: where the idea stands

A hand-off for a fresh session. Nothing here is built; it is what was decided and what was
learned while deciding, so the next session starts from it instead of from nothing.

## What the owner wants

**Spatiand on the Mac is a menu-bar app, and the glasses are optional.**

* **No separate screen.** A Mac (like the Beam Pro) already has a screen, a keyboard and a
  mouse, so no sidecar display is needed. Spatiand lives as an icon in the menu bar. Its menu
  shows what the Deck's sidecar shows -- state, computers, windows -- and the **sound output
  choice**.
* **Without glasses**, everything Spatiand shows -- applications from a remote computer, and
  later native Spatiand apps -- is an ordinary window on the Mac, beside the Mac's own windows,
  which stay exactly where they are. Sound is flat stereo. This makes the remote protocol
  useful with no glasses at all.
* **With glasses plugged in**, the same windows go into the 3D world, placed and curved like
  the Deck's, with spatial sound. Unplug them and the windows come back to the Mac.
* **VR applications** (a Second Life viewer, SpatiWorld) are told to switch to a non-VR mode
  when there are no glasses, and back to VR mode when there are. That is a message in the
  protocol and a mode in the application, not something the compositor can fake. Without
  glasses the sound of such an application is flat stereo.
* **The clipboard works across machines**, both ways, as it does on the Deck
  (`crates/spatiand-stream/src/clipboard.rs`).
* **Stretch:** the Mac shows its own windows and sound per app, as Linux does. See "What was
  learned about capture" for why that is the hard way.

### Input

A Mac has no dual trackpad and no STEAM / `⋯` buttons, so they are emulated:

* the mouse and trackpad are *taken by Spatiand* while it has the pointer, and projected into the
  window, cursor included;
* two-finger scroll is scroll (the zoom of a window);
* three, four and five finger gestures act on the projected virtual screen;
* **Ctrl-Space** opens the Spatiand menu (the launcher, `⋯` on the Deck);
* **Ctrl-Tab** opens the settings (the HUD, STEAM on the Deck).

### One idea, two presentations

The same session code serves both. What differs is where a window is drawn: in the glasses'
world (when plugged in) or in an `NSWindow` (when not). A window's picture, sound, input and
clipboard travel the same way either way, so the Mac client is the remote protocol's client
with two ways of showing what arrives.

## Architecture (proposed)

* **Swift for the Mac front end:** the menu-bar icon, windows, settings, audio-device choice and
  glasses detection. Decoded video goes to an `AVSampleBufferDisplayLayer` (hardware decode and
  display from the stream's access units with no copying), sound to `AVAudioEngine` on the chosen
  device.
* **Rust for the link:** `spatiand-stream` (QUIC, pairing, framing, clipboard, catalogue)
  already builds and tests on the Mac and is shared with the Deck and the host. The Swift app
  would call it through a thin C interface rather than re-implement the protocol.
* Not decided: whether the 3D world on the Mac reuses the Rust renderer or HoloFrame's Metal
  one.

## Status

`mac/` is a Swift package; `crates/spatiand-mac-core` is the Rust link it calls. `mac/build.sh`
builds both. This Mac is paired with the owner's host and everything below was run against it.

**Works (each checked end to end with a headless self-test, `Spatiand --selftest-*`):**

* Pairing from the menu (this Mac shows the code, the host is asked, and the host is written
  down only after both have said yes).
* A host's applications as ordinary Mac windows: HEVC/H.264 decoded by VideoToolbox and shown by
  an `AVSampleBufferDisplayLayer`; the mouse, wheel and keyboard go back (Command stands in for
  Control, switchable); resizing asks the host's window to resize; closing asks the application
  to close.
* A window that was already running when the session arrived is shown (its first keyframe is
  kept if it beats the window, and a window with no picture asks again each second).
* The clipboard both ways: text (small travels with the offer) and PNG images (fetched when
  announced, up to 8 MB). Mac to host was seen arriving in the host's log.
* Sound from a host, folded to flat stereo and played into the chosen output device.
* The host's cursor shape; the menu-bar menu; a settings window; Ctrl-Space (the menu) and
  Ctrl-Tab (the settings) as global hot keys. Both chords were granted on the owner's Mac;
  Ctrl-Space is the system's input-source switch if that is turned on, and Ctrl-Tab is taken
  from every other application while Spatiand runs, so both can be switched off.

**Written but not seen working:** menus and tooltips as child panels (a menu was not seen
opening in the harness, so the placement is unproven); the cursor shape; the sound itself (the
fold is tested, nothing was played at the owner's sleeping house).

**Not built:**

1. The two-mode message for VR applications (see below), and per-app audio at the source.
2. The glasses path: with glasses plugged in the windows should be in the 3D world. Today the
   "Show windows" setting is stored and nothing reads it.
3. Taking the mouse and trackpad away from macOS, and the three-to-five-finger gestures.
4. The Mac as a *host* (its own windows out to a Deck or Beam Pro).
5. A signed `.app` bundle; today it runs from `.build/`.

## Where the code to start from is

* HoloFrame (a separate Swift project of the owner's): the macOS XREAL head tracker the Rust
  tracker came from, and the virtual display work. Its `HeadTracker.swift` is the origin of
  `crates/spatiand-track/src/tracker.rs`; check its history before touching drift.
* `spatiand-render` and `spatiand-shell` already build and test on the Mac. The compositor
  crate (`spatiand`) needs smithay and does not; the Beam Pro port (`crates/spatiand-android`)
  shows how the session sources are shared by `#[path]` for a platform with no compositor.
* `spatiand-host` and `spatiand-stream` are the remote-app protocol. A Mac could be a host
  (pictures out) or a client (pictures in); see `docs/remote.md`.

## What was learned about capture on macOS

* ScreenCaptureKit hands over GPU buffers (`IOSurface`) with no CPU copy, so capturing a window
  or display *on the same machine* costs little: no encode, no decode. Cost grows with how many
  windows are changing and how large they are. Encode/decode only appears across a network.
* It captures windows that are covered, but not minimised or hidden ones.
* It can capture the audio of chosen apps, before the system mix, as stereo only. Whether the
  Mac can be silenced meanwhile (output to a null device) is **untested**.
* **Untested:** the probe at `mac/tools/sckaudio.swift` (build with `swiftc -O sckaudio.swift
  -o sckaudio`; it has a `list` and a `capture <bundle id> <seconds> <channels>` command). It needs Screen Recording permission for whatever
  launches it, and the assistant's own shell was refused with error -3801 and no prompt. Run it
  from Terminal.
* Virtual displays on macOS rest on an interface Apple has not formally documented (what
  DeskPad and BetterDisplay use). HoloFrame already creates one.

## Cautions for the input design

* **Ctrl-Space** is macOS's default shortcut for switching input source, and **Ctrl-Tab** is
  next-tab in browsers and many apps. Taking either globally means an event tap, and an event
  tap needs the Accessibility / Input Monitoring permission. Decide whether to capture them only
  while the glasses are active, and what the escape is when something goes wrong.
* Taking the pointer from macOS likewise needs an event tap (or a transparent full-screen
  capture window). Keep an unmistakable way back to the real cursor.
* Three-to-five finger gestures are already macOS's own (Mission Control, Spaces, Launchpad);
  they will need to be suppressed or remapped while Spatiand has the pointer.

## Open questions

1. Does the Mac also act as a *host* (its own windows out to a Deck or Beam Pro), or only as a
   client of other computers' applications? The menu-bar design above is the client.
2. The 3D world on the Mac: the Rust renderer, or HoloFrame's Metal one?
3. How the VR/non-VR switch is carried: a new message in `spatiand-stream`, and what an
   application has to do to honour it (SpatiWorld is the first).
