# Spatiand on the Mac: the room is the Deck's compositor

On 2026-10-09 the Mac's room was rebuilt. Before, it was a second implementation: a Metal
renderer, menus, title bars, a pointer and a controller mapper written again in Swift and Rust,
which had to be made to match the Deck feature by feature and never quite did. Now it is built
the way the Beam Pro's is: **`crates/spatiand-mac` compiles `crates/spatiand`'s own modules by
path**, so the room, the launcher, the settings, the pointer and its resize cursors, the window
frames, the on-screen keyboard, the environments, the controller layouts and the status bar are
the Deck's code, not a likeness of it.

```
 Spatiand.app (Swift)                       libspatiand_mac.a (Rust)
 ┌─────────────────────────────┐            ┌───────────────────────────────────────────┐
 │ GlassesScreen  a window on  │ sp_glasses │ mac/egl.rs      ANGLE (GLES on Metal),     │
 │                the glasses  │───────────▶│                 a CALayer, a display link  │
 │ RoomTap        mouse, keys, │ sp_pointer │ mac/controller  the Deck's controller      │
 │                trackpad     │ sp_key     │                 state, from a Mac's devices│
 │ RoomWindows    capture, and │ sp_window_*│ mac/local.rs    each Mac window an ordinary │
 │                real clicks  │◀───────────│                 window, via remote::client │
 │ PadInput       controllers  │ sp_pad_*   │ mac/pads.rs     GamepadState, as on Android│
 │ Notices        banners      │ sp_notice  │ scene.rs        the card under the clock   │
 │ MacSound       app taps     │sp_app_sound│ spatiand-audio  server_fed.rs + Core Audio │
 │ RoomMenus      an app's own │ sp_menu_*  │ spatiand-shell  a list, like the settings  │
 │                menus        │            │                                            │
 │ MacScreens     displays, a  │ sp_window_*│ mac/local.rs    a screen is a window       │
 │                virtual one  │            │                                            │
 └─────────────────────────────┘            │ ── and the frame loop itself:              │
   2D mode, pairing, the Mac as a host:     │    spatiand-android/src/android/backend.rs │
   unchanged (spatiand-mac-core)            └───────────────────────────────────────────┘
```

## What is shared, and how

* **The frame loop is Android's file**, `crates/spatiand-android/src/android/backend.rs`,
  compiled into both. What differs between a Beam Pro and a Mac is behind names each platform's
  module supplies (`super::egl`, `super::controller`, `super::pads`, `super::take_glasses`,
  `super::local_apps`, `super::notice`, …): see the table at the top of
  `crates/spatiand-mac/src/mac/mod.rs`.
* **Smithay** builds for macOS with a handful of Linux-only calls patched in corners a Mac never
  runs (X11 sockets, DRM sync files, a sealed keymap file, the external-image shader):
  `mac/vendor/smithay`, used only by `crates/spatiand-mac`, which is a workspace of its own so
  that the Deck and the Beam Pro go on building crates.io's.
* **ANGLE** is the GLES driver. `mac/fetch-angle.sh` copies a universal (Intel + Apple Silicon)
  build from an installed Electron application into `mac/vendor/angle` (not committed).
  `mac/xkbcommon/build.sh` builds libxkbcommon for both architectures.
* **Pictures** are `IOSurface`s bound as textures with no copy (`mac/remote_video.rs`), behind
  the same placeholder-and-token handover Android uses: a host's window decoded by VideoToolbox
  (`spatiand-video/src/mac_decode.rs`), or a Mac window captured by ScreenCaptureKit.
* **The glasses** are the Deck's driver over IOKit (`spatiand-hmd/src/iohid.rs`).
* **Sound** is the Deck's engine (`spatiand-audio/src/server_fed.rs`, shared with Android) on a
  Core Audio output unit.

## Three things macOS does that cost a day each

1. **A frame with alpha 0 is invisible.** The scene leaves whatever alpha blending comes to; a
   display ignores it and the window server does not. `egl::finish_frame` sets it to one.
2. **ANGLE attaches its Metal layer from the compositor's thread, at its first present**, and
   that thread has no run loop to commit the change: every frame was drawn into a layer the
   window server had never heard of. `egl::pace` flushes after each of a new window's first
   frames.
3. **ANGLE's swap does not wait for the display.** The frame clock is a `CVDisplayLink` on the
   glasses' display (`egl::follow_display`, `egl::pace`).

## Building and looking at it without glasses

```
mac/build.sh                 # debug: Rust, then Swift
mac/make-app.sh              # release, signed, installed at ~/Applications/Spatiand.app

# One frame of any menu or view to a PNG -- the Deck's snapshot tool, on a Mac:
cd crates/spatiand-mac && SPATIAND_VIEW=launcher SPATIAND_VIEW_APPS=60 SPATIAND_VIEW_TYPE=co \
    SPATIAND_SNAPSHOT=/tmp/l.png cargo run

# The real session with stand-in glasses, in a window on the Mac:
mac/.build/debug/Spatiand --preview 20 [/Applications/Some.app]
mac/.build/debug/Spatiand --selftest-click      # the room's pointer clicks Calculator
mac/.build/debug/Spatiand --selftest-windows    # every Mac window in the room's list, hidden
mac/.build/debug/Spatiand --selftest-notice-room
```

`--preview` and the self-tests use their own preferences (`$TMPDIR/spatiand-preview`) and do
not connect to hosts. `--selftest-click` moves the real pointer: not while the Mac is in use.

## In the glasses

* Control-Option-G switches the mouse and keyboard between the room and the Mac's own screen.
  Control-Space is the launcher, Control-Tab the settings, Control-Option-R recentres. While the
  room has the mouse the Mac's own pointer is hidden.
* The launcher lists this Mac (by name) and each paired computer; a machine opens to all its
  applications by name, and typing narrows them. Opening a Mac application brings its windows in.
* **The settings' Windows list** has every window open on the Mac, put away, under
  *Applications*; choosing one brings it out. Only windows on show are captured. A window opened
  while the room is going comes out at once. A tab is not a window: a new tab, or another tab
  chosen, is the same window in the room showing something else.
* **Screens** are in the same list, above the applications: each display the Mac has but the
  glasses, and a virtual one. A screen is a window whose picture is everything on that display
  and whose pointer is the Mac's real one. The virtual screen is a `CGVirtualDisplay`
  (`mac/Sources/CSpatiand/SPVirtualDisplay.m`, from HoloFrame): macOS takes it for a monitor, so
  windows can be put on it and it is arranged in System Settings. It is made the first time it is
  brought out, at the size chosen in Spatiand's settings (Glasses → Virtual screen), and lasts
  until Spatiand quits.
* What is done inside a Mac window is done to the real window with the real pointer, with its
  application in front -- except a press on something the application has put up itself (a
  browser's list of profiles), which bringing the window forward would close. A drag is the
  hand's own events, passed through. Scrolling and pinching over one are left to macOS itself.
* What hangs beside a window -- a menu dropped from a toolbar, a tooltip past the edge -- is in
  its picture, and the corners that are neither are seen through.
* **A menu button** at the left of a Mac window's bar opens its application's own menus as one of
  the room's lists, read and pressed through Accessibility (`RoomMenus.swift`): the Mac's menu
  bar is on the Mac's screen, out of sight.
* **Sound.** An application with a window in the room is tapped (a Core Audio process tap) and
  heard from where its window is; the speaker on the bar mutes it, and scrolled or slid it is that
  window's own volume, shown as a ring. A screen on show has the sound of everything else.
* Pinching on a title bar moves the window nearer or further; three fingers drag; four fingers
  swipe up for the launcher, down to back out, and tap to recentre. Two fingers, or a wheel,
  move through an open menu.
* *Sound devices* in the settings chooses the output and the microphone; *Record a video* films
  the glasses' view (macOS 15) to `~/Movies/Spatiand`.
* The view is held steady against the wearer's pulse (`spatiand_track::Steady`,
  `SPATIAND_STEADY=0` to compare).
* From a Mac, Command is sent to a host as Control.

## The Mac as a host

Switched on in Spatiand's settings (Computers), and paired from its menu. To a Deck or a Beam
Pro it is a host like the Linux one, with these differences, which are a Mac's:

* **Its windows are already open, on a screen of its own.** So at attach each is *offered*
  (`HostMessage::Offered`) and listed put away in the device's list of windows; nothing is
  captured until one is asked for (`ClientMessage::Show`). A window opened afterwards is shown at
  once. Its screens are offered the same way (`OfferedScreen`), and a screen that is closed is on
  offer again.
* **What a device does in a window is done with the Mac's real pointer**, the window brought
  forward first: an event addressed to an application in the background is ignored by anything a
  browser's engine draws.
* A screen is sent at the size it looks, not at the size macOS draws it before scaling it down,
  and with the sound of everything that has no stream of its own.
* The Mac's notifications are sent to the device and shown under its clock.

## Tests that need no glasses

All are modes of the app itself; what they print is in `~/Library/Logs/Spatiand/spatiand.log`
when run from the bundle. **The ones marked ✋ move the Mac's real pointer.**

| | |
|---|---|
| `--selftest-pointer` ✋ | points at and clicks a window of its own, sweeps, drags, resizes it; `--popup` clicks in a bubble hung beside it |
| `--selftest-child [--goes] [--tab]` | a window with something beside it, which then goes; or with a second tab chosen |
| `--selftest-terminal-tabs <title>` | switches the tabs of a real Terminal window |
| `--selftest-menu <App>` | opens an application's menus as a list, and one level of them |
| `--selftest-app-sound <App>` | takes a playing application's sound for the room |
| `--selftest-screen [--virtual]` | brings a screen into the room |
| `--selftest-windows [--and-choose]` | the list of windows |
| `--selftest-hover <App> <title>` | moves over a window as the room does |
| `--picture-settings <file>` | the settings window, to a PNG (`SPATIAND_SETTINGS_PAGE=0…4`) |
| `--serve-host <s>` | this Mac as a host only; `crates/spatiand-stream/examples/probe-session` connects to it (`--show-screens`) |

## Checked on real glasses, and not

On an Intel MacBook Pro with XREAL Air glasses, in use: the room at 60 Hz in two eyes, the
pointer, clicks, drags and resizes in Mac windows, tabs, the menu button, a browser's pop-ups,
sound from an application's window, and a host's windows.

Only with stand-in glasses in a window, or only against a test client: a screen in the room and
its pointer; a device using this Mac's screens; the virtual screen at sizes other than
1920 × 1080; a HiDPI screen sent at the size it looks. Not tried at all: an Apple Silicon Mac.

**The glasses run at 60 Hz on a Mac**, not the Deck's 72: macOS does not offer the two-eye
mode at 72.

## Not done

* Game controllers in windows on this Mac without glasses (they went through the removed
  `pads.rs`); in the room they work through the Deck's mapper.
* Five-finger gestures; four-finger sideways.
* The microphone to a host (`spatiand-video`'s `voice` is a stub on the Mac, as on Android).
* Notifications on the Beam Pro: `android::notice()` returns nothing; the card and its press
  are in the shared scene and loop.
* The settings' *Sound devices* page is the Mac's only.
