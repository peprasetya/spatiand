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

**Added afterwards the same week:**

* **Title bars.** Every window in the room has a bar above it (`RoomTitles.swift`; the room's maths gives it a hit
  zone of its own): the window's name, a button to pin it to the glass and one to close it. Taking the bar carries
  the window round the room. A window pinned to the glass has none.
* **Trackpad gestures** (`GestureRecognizer.swift`, tested in `LocalTests`; the raw touches come from the key window,
  no private interface). Three fingers: swipe sideways brings the next window here, up opens the menu, down puts it
  away, a tap recentres. Four: swipe moves the window pointed at, pinch resizes it. Five: pinch gathers every window
  in front of you, spread gives them room. macOS keeps its own meanings for these (and has three-finger drag on here),
  so while three fingers are down the pointer and its button presses are the gesture's, and the settings list which
  system gestures are on, for the owner to turn off.
* **A measured head.** The app carries libmysofa and the KEMAR dataset it ships (`tools/build-hrtf.sh`, built once
  and put in the bundle by `make-app.sh`; licences in `Resources/hrtf/NOTICE.txt`), so a sound can be put behind you.
  Without them, or when run from the build folder, it falls back to the parametric head.

**Added after that:** resizing a host's window from the room (drag its bottom right corner, press the fit button on its
title bar or in the menu, or make it bigger with the four-finger pinch: the host is asked for that many pixels at the
density windows start with, and the window grows with the answer rather than stretching; a Mac window is resized through
Accessibility), and sound from the Mac's own applications when the Mac is the host (ScreenCaptureKit captures one
application's sound, sent as the same raw stereo stream the Linux host sends; tested over loopback). The Mac still
plays that sound itself too: there is no way to take it from the speakers and leave it in the capture.

**In the glasses, as on the Deck** (after the first real use of the glasses showed the room was a different program from
the Deck's). The same look and the same controls, taken from the Deck's code rather than invented again:

* **The environment.** The room was black, which on the glasses reads as flat: nothing said how far away a window was. It
  now has the Deck's generated studio (`Sky::studio`: a dark sky, a key light, a horizon and a floor grid) behind it.
  Settings has `studio` on by default; "Environment" in the settings list switches it to black.
* **Window frames.** `crates/spatiand-mac-core/src/chrome.rs` is the Deck's `pointer.rs` `Frame`/`Zone`/`resize` and the
  drawing of `scene.rs` (`look.rs` holds the copied glyphs): a pane of glass round the window with the application's icon,
  its title, and buttons for close, put away, pin to the glass and (only for a window making a sound) mute. A press on
  an edge resizes the window at constant pixel density with the opposite edge fixed; the title bar moves it.
* **Distance.** A pinch over a window's frame or title bar brings it nearer or pushes it further (0.8 to 8 m), and the
  nearer one is drawn over the further and is the one aimed at. A pinch over its contents is the application's: Control
  and the wheel to a host window, Command and plus or minus to a Mac window. Option and the wheel, and the
  four-finger pinch, also move the window in distance.
* **Menus.** `spatiand-shell` runs unchanged inside the room (`shell_ui.rs`, with the Deck's `menu.rs` copied as
  `menu_model.rs`): Ctrl-Space is the launcher (the Deck's ⋯), Ctrl-Tab the settings list (STEAM), a pad's PS button the
  settings and held the launcher. The arrows and Return, the D-pad and A, or the pointer work them. The launcher has a
  bubble for each computer, and one for This Mac whose applications bring their windows into the room. Every row of the
  settings list does what it does on the Deck (see "Parity with the Deck" below); only calibration, which the Mac learns
  as the glasses are worn, says so in a panel.
* **Yaw drift.** The Deck and the Beam Pro hand the tracker what it learned about the glasses' sensors last time (the
  gyro's resting offset and the magnetometer's own field, `SensorMemory`) and the Mac never did, so it started without a
  bias and without the magnetic anchor that holds yaw. It does now, kept in `~/.config/spatiand/sensors.toml`. The Rust
  half also had no logger, so the tracker's own account of the anchor went nowhere: it writes to the app's log now
  (`~/Library/Logs/Spatiand/spatiand.log`, a `tracker:` line every thirty seconds). The anchor needs a minute or two of
  looking up, down and round the first time (the same glasses' file can be copied from the Deck's
  `~/.config/spatiand/sensors.toml`, which already has it; the app no longer interrupts to ask).
* **Launcher glass.** The bubbles are the Deck's refracting glass (`fragment_bubble` in `RoomRenderer.swift` is the
  Deck's `BUBBLE_FRAG`), with the application's icon inside, its name on a plate below, and the computers and groups
  drawn from their icon-theme names by `theme_icons.rs` (a Mac has no icon theme). The focused one is lit.
* **Launcher search.** A Mac has some two hundred applications, which is seventeen pages of bubbles and not a
  thing to categorise, so the launcher is a search. Typing narrows the bubbles as you go (`Launcher::type_query` in
  `spatiand-shell`, so any front end can use it): names that begin with the letters first, then a word that does,
  then a name that contains them, then the letters in order ("vsc" finds Visual Studio Code), accents ignored. From
  the top it looks through every computer's applications, inside one computer or group only that one. The arrows move
  among what is left, Return opens it, Escape takes back the search and then the launcher, and the launcher opens
  blank. The field is a glass pill at the top of the view (`search_image`, `shell_ui.rs`) with how many it caught;
  the rows sit a little closer to make room for it, and a long list says "3 / 17" in words, as seventeen dots do not
  fit the field's height. It does not raise the Deck's on-screen keyboard (`Shell::searches`, not `wants_text`).
  `Spatiand --selftest-launcher` drives it through the menu's own key handling and writes what the glasses would show
  to `/tmp/spatiand-launcher-*.png`.
* **The pointer** is the Deck's reticle, and over a window's frame the Deck's double arrow, turned to lie along the
  way that edge or corner pulls.
* **A Mac window with the keyboard is made the active one.** The Mac shows a caret, keeps a popover open and selects
  text only in the active application, and an application that is merely posted events is not. So while a Mac window is
  the one in front of the wearer, its application is activated and its window raised (`RoomInput.giveKeyboard`), the
  keys go to it directly, and Spatiand's input window is a non-activating panel that still hears the frozen pointer, the
  wheel and the pinch. Spatiand's own chords (Ctrl-Option and G, R, P, B, C, S, W) are then registered with the
  system, and opening a menu or focusing a host's window takes the keyboard back. `defaults write
  com.peprasetya.spatiand activateMacWindows -bool false` goes back to posting keys at a window in the background.
* **Cost.** The window in use is captured at the display's rate and the rest at half; the pointer is told of a move at
  most every 25 ms (8 ms when dragging) and always of the last; a captured window is copied into a mipmapped texture so
  text does not shimmer when the head moves.

**Game controllers** (a DualShock 4 over a cable or Bluetooth, a DualSense, an Xbox pad) work as they do on the Deck and
the Beam Pro: `mac/Sources/Spatiand/PadInput.swift` reads them with GameController, and `spatiand-mac-core/src/pads.rs`
runs them through the same `spatiand-mapper` engine and layout files. A host window gets the pad as a gamepad (and
rumble comes back); the desktop layout types the D-pad as arrows, A as Enter and B as Escape; in the room the touchpad
slides the pointer and its press clicks, the triggers click (right is the left button), A, B and X click while a thumb is
on the touchpad and the layout has not claimed them, the sticks scroll, and the PS button opens the menu, which the D-pad,
A and B then work. GameController gave no touchpad on this Mac, so `PadTouchpad.swift` reads the DualShock 4's own
reports through IOHID beside it (checked with a pad on a cable; Bluetooth is switched to the long report the way drivers
do, unchecked). `Spatiand --selftest-pad <seconds>` prints what a pad says. Layouts saved on a Deck work when copied to
`~/Library/Application Support/Spatiand/layouts`; there is no layout editor here. Not done: the glasses' gyro as a source
for layouts, the on-screen keyboard and screenshot commands, and the layout editor.

**Not built:**

1. Notarisation: `mac/make-app.sh` installs `~/Applications/Spatiand.app`, signed with HoloFrame's self-signed
   "HoloFrame Dev" certificate when it is in the keychain (so the Screen Recording and Accessibility grants survive
   rebuilds), ad hoc otherwise. Not notarised, so for this Mac only.

### Parity with the Deck

Everything the Deck's settings list offers is there, by the Deck's own code where it has any, which was moved into
`spatiand-room` so the Deck, the Beam Pro and the Mac use one copy of it:

| The Deck's | On the Mac |
|---|---|
| Environment: black, the generated studio, any picture in the folder, add one from a file browser, remembered | `spatiand_room::environment` (the same module, moved), `surroundings.rs` joins it to the menus; the picture is drawn by `fragment_sky` with the eye's part of a stereo pair, the front half of a 180, and the turn. Same folder (`~/.local/share/spatiand/environments`) and state files; Settings has a button that runs `tools/fetch-environments.sh` for the default NOIRLab panoramas |
| Status line (time, battery, windows), head-locked upper left | `spatiand_room::status` for the words, `StatusLine.swift` for the battery, `Room::locked` for holding a panel to the head |
| Controller layout editor and its picture | `spatiand_mapper::editor` through `Pads`, drawn as the Deck's card with `spatiand_room::diagram` above it |
| Radial menu of a layout | the mapper's `RadialView`, a ring of head-locked plates |
| On-screen keyboard under the window being typed into | `spatiand_shell::keyboard` and `spatiand_room::keyboard_face`; a press is typed into the window in front |
| Record a video | `Recorder.swift`: the room drawn again into the encoder's buffer, and what the Mac is playing, to `~/Movies` |
| Wi-Fi and Bluetooth panels floated as windows | System Settings' own panes, brought into the room like any Mac window |
| Sound of each window placed where it is | `MacTap.swift` / `MacSound.swift`: a Core Audio process tap, muted on the speakers while taken. Off until asked for in Settings, because macOS asks permission the first time and an application is silent on the Mac while its sound is held |
| Calibrate head tracking | not needed: the Mac learns the glasses' sensors as they are worn and keeps them |
| Sidecar screen, volume rocker, power button | the Deck's own hardware; the Mac has its own screen and keys |

What is still different: the keyboard cannot be dragged larger by its frame, a hovered key does not stand up, and the
idle fade the Deck gives a film's transport bar is not there.

Tests that drive these without glasses (each writes pictures of what the glasses would show to `/tmp`):
`Spatiand --selftest-launcher`, and, through `tools/selftest-scratch.sh` (which keeps the wearer's own folders out of it),
`--selftest-environment`, `--selftest-controller`, `--selftest-keyboard`, `--selftest-record` and `--selftest-tap`.

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
