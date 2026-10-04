# Spatiand on macOS: where the idea stands

A hand-off for a fresh session: what was decided, what was learned, and what is built.

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

* **The glasses message.** The Mac tells the host whether the wearer has glasses on
  (`ClientMessage::Glasses { on }`, last in the enum), when it connects and whenever they are
  plugged or unplugged, or the "Show windows" choice changes. A host assumes `on` until told, so
  the Deck and the Beam Pro are unaffected. When it is `off` the host tells each virtual-reality
  application on its control socket, in the same words it uses for the render size:
  `set_glasses 0`, and `set_glasses 1` when they come back (see `appcontrol.rs`). Seen: the
  host told Firestorm `set_glasses 0` as it launched. **No application acts on it yet**:
  honouring it -- drawing an ordinary flat view -- is up to the viewer (SpatiWorld first).

* The keyboard, seen: Down three times and Return, sent as evdev codes the way a key press is,
  moved the host's settings app to another page.

**Written but not seen working:** the cursor shape; the sound itself (the fold is tested, nothing
was played); the pointer capture. **Menus:** the host composites a window's menus into that
window's own picture and never announces them as windows (`parent` is always `None`), so they
appear with the window and the Mac's popup panels (`RemotePopup`) are unused until a host
announces one. A Qt menu did not open from a synthetic click in the harness, and the terminal
(an X11 Qt program) did not take synthetic keys -- both look like that application's behaviour on
the host, and the Deck would see the same.

**The glasses path (built 2026-10-05, tried on the real glasses).** With glasses plugged in and
"Show windows" on its default, the windows leave the desktop and go into the room:

* The glasses are put into their side-by-side mode (3840x1080) -- they re-enumerate, offer the
  mode, and macOS has to be asked for it for the session -- and their display is taken over with a
  Metal window, a frame every refresh. If no 3D mode turns up it falls back to one eye.
* The head comes from the glasses' own IMU over USB HID (`XRealDevice.swift`, from HoloFrame) into
  the **same tracker the Deck uses** (`spatiand-track`, via `crates/spatiand-mac-core/src/room.rs`).
  The temple button recentres.
* Where windows are, what the pointer is over, what to draw, and the pinned-to-the-glass window
  are Rust (`room.rs`, reusing `spatiand-render`'s ray, bend and picture-in-picture maths) and are
  unit-tested. Each window is decoded with VideoToolbox, textured onto strips bent round the
  wearer (the Deck's own geometry), and drawn once per eye.
* The Mac's mouse, trackpad and keyboard are taken (a transparent window; no Accessibility
  permission) to steer a cursor in the room: clicks, the wheel and keys go to the window pointed at.
  Option-drag moves a window, Option-wheel or a pinch resizes it. Ctrl-Option-G gives the Mac its
  input back, and so does switching to another app. Other chords: R recentre, P pin to the glass,
  B bring here, C and S the pinned corner and size, W close the window.
* Ctrl-Space (Ctrl-Tab too) opens a menu *in the glasses*: what the host can open, which windows to
  bring here, recentre, pin, close, the pinned corner and size, the video limit.
* Sound is placed where its window is, by the Deck's own binaural renderer (`spatiand-audio`:
  stage directions, the head-related filters or the parametric panner, the plain blend), following
  the head; 5.1, 7.1 and 7.1.4 from the host are placed channel by channel.
* Windows of this Mac can be brought into the room too (the menu: "Windows of this Mac"): captured with
  ScreenCaptureKit and drawn like any other, with no encode or network. Clicks, the wheel and keys are posted at the
  window's application with `CGEvent.postToPid` and the window named in the event, so the window need not be in
  front. **Needs Screen Recording to show and Accessibility to click; the click path is untested** (the tool that
  built it is not trusted to post events).
* Unplugging, quitting, and `kill` all give the glasses their ordinary mode back and the Mac its
  pointer; unplugging restarts the app so the next plug-in starts clean (HoloFrame's lesson: a
  vanished display leaves a ghost window in the window server).

`Spatiand --selftest-room <address> <fingerprint> <app>` draws the room offscreen with no glasses
(`/tmp/spatiand-room*.png`); `SPATIAND_DEBUG_SNAPSHOT=/tmp/g.png` on the app writes what the glasses
are showing every few seconds.

**The Mac as a host (built 2026-10-05, tried over loopback only).** The menu's "Let other devices use this
Mac's windows" makes this Mac listen on port 47600, speaking the Linux host's protocol, so a Deck or a Beam Pro can
pair with it (the menu opens pairing for two minutes; this Mac asks to confirm the six digits) and open its running
applications: the transport is Rust (`host.rs`: QUIC, a gate for paired devices, the control stream, packetised
pictures), the rest is Swift (`MacHost.swift`: ScreenCaptureKit capture, `HostEncoder.swift` hardware HEVC, input posted
back at the application, close and resize through Accessibility, the clipboard). `Spatiand --selftest-host
[bundle id]` has the client half of the same app connect to the host half over 127.0.0.1 and photograph what comes
back: three Terminal windows arrived with real pictures. **Not tried:** a real Deck or Beam Pro, and clicking or typing
(they need Accessibility, which the tool that built it does not have). A session's Control key is made Command
(not in terminals), so copy and paste feel native; sound from the Mac's applications is not sent.

**Not built:**

1. Applications honouring `set_glasses` (SpatiWorld first: see `spatiworld-glasses.md`), and per-app
   audio at the source on the Mac.
2. Three-to-five-finger gestures on the projected screen (macOS keeps them for itself; they need
   the private multitouch interface), and a measured head-related dataset (the parametric panner is
   used: libmysofa is not on this Mac).
3. Window frames in the room (title bars, close and resize handles): windows are moved, resized and
   closed with chords and the menu. The host window's own size does not follow the wearer's resizing.
5. Notarisation: `mac/make-app.sh` installs `~/Applications/Spatiand.app`, signed with HoloFrame's self-signed
   "HoloFrame Dev" certificate when it is in the keychain (so the Screen Recording and Accessibility grants survive
   rebuilds), ad hoc otherwise. Not notarised, so for this Mac only.

## The Mac as a host (later)

The same protocol the other way round, for sending a Mac's windows to a Deck or a Beam Pro. It
would be a *capture* host rather than a compositor: choose a window or an app from the menu,
capture it with ScreenCaptureKit (zero-copy `IOSurface`), encode with VideoToolbox's hardware
encoder, and serve it through `spatiand-stream`'s transport, pairing and catalogue exactly as
`spatiand-host` does. Input comes back as `CGEvent`s (needs Accessibility permission), sound is
the app's own, captured by ScreenCaptureKit as stereo, and the clipboard is the same pasteboard
code this client already has. None of `spatiand-host`'s code applies (it is a Wayland
compositor); the protocol, the pairing and the clipboard rules do.

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
