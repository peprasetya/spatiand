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
  Control-Space is the launcher, Control-Tab the settings, Control-Option-R recentres.
* The launcher lists this Mac (by name) and each paired computer; a machine opens to all its
  applications by name, and typing narrows them. Opening a Mac application brings its windows in.
* Every window open on the Mac is in the settings' Windows list, put away; choosing one brings
  it out. Only windows on show are captured.
* What is done inside a Mac window is done to the real window with the real pointer, with its
  application in front. Scrolling and pinching over one are left to macOS itself.
* Pinching on a title bar moves the window nearer or further; three fingers drag; four fingers
  swipe up for the launcher, down to back out, and tap to recentre.
* From a Mac, Command is sent to a host as Control.

## Not checked on real glasses yet (2026-10-09)

Everything above was run with stand-in glasses in a window. With real ones, untested: the
IOKit transport and the switch to two eyes; the window on the glasses' display and its pacing
at 72 Hz; the event tap while wearing them; trackpad gestures; sound taps; a host's windows
decoded by the new VideoToolbox path.

## Not done

* Game controllers in windows on this Mac without glasses (they went through the removed
  `pads.rs`); in the room they work through the Deck's mapper.
* Recording the glasses' view (`mac/record.rs` is a stub).
* Five-finger gestures; four-finger sideways.
* The microphone to a host (`spatiand-video`'s `voice` is a stub on the Mac, as on Android).
* Notifications on the Beam Pro: `android::notice()` returns nothing; the card and its press
  are in the shared scene and loop.
