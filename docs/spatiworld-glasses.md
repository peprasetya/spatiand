# SpatiWorld: being a window when there are no glasses

A hand-off for the session that works on SpatiWorld (the Firestorm fork at
`/media/shared/Linux/App/SpatiWorld` on deepmagpie). **Done since this was written:** the viewer side is in the SpatiWorld tree (commits "Be an ordinary window when the
session has no glasses on" and "Remember a set_glasses that arrives before login"). Checked from the Mac app on
2026-10-04: a session with no glasses launched SpatiWorld and its log said "the session has no glasses on; starting as
a window". The text below is the design it was built from.

## What is wanted

The Mac client (and later any client without a head) can show remote applications as ordinary
windows. A viewer that is "the room" -- two eyes side by side, a projection layer, its own
pointer -- is no use in a window on a laptop. So when the session has **no glasses on**, the
viewer should become an ordinary flat Second Life viewer: one eye, a normal window, the normal
pointer and keyboard, no head turning the camera. When glasses come back, it becomes the room
again, **without restarting**.

## What the host already does

`ClientMessage::Glasses { on }` (`crates/spatiand-stream/src/control.rs`, last variant) says
whether the session has glasses on. The host assumes `on` until told, so the Deck and the Beam
Pro never say anything and nothing changes for them. For a Mac with no glasses:

1. **At launch:** a `kind = "vr"` application started while the session has no glasses gets
   `SPATIAND_GLASSES=0` in its environment (`main.rs`, next to `apps::launch`). Absent means
   glasses on. Read it in `SpatiandStereo::detect()` and start flat -- no flash of the room.
2. **While running:** the host writes, on the control socket the viewer already reads
   (`SPATIAND_CONTROL_FD`, a `SOCK_SEQPACKET`, one message per datagram):

   ```text
   set_glasses 0     the session has no glasses: be a window
   set_glasses 1     the glasses are back: be the room
   ```

   Only a change from the default is ever sent, and it is sent once per change
   (`crates/spatiand-host/src/appcontrol.rs`, `glasses_word`). Today the viewer's
   `SpatiandStereo::listen()` (spatiandstereo.cpp, around line 518) logs
   `spatiand-host said something this viewer does not know: set_glasses 0` and carries on in
   VR -- that line in `~/.spatiworld_x64/logs/SpatiWorld.log` is how to see it arrive.

## What the viewer has to do

On `set_glasses 0` (and at start with `SPATIAND_GLASSES=0`), leave stereo:

* `sEyes = EYES_MONO`, so `isStereo()` is false. `eyeCount()`, `currentEyeShift()` and the canvas
  arithmetic already follow it.
* Say what it has become, in the same words it used to say it was the room:
  `tell("set_eye_layout mono")`, `tell("set_layer window")`, `tell("set_cursor_drawn 0")`. The host
  passes them on, and the session then treats the viewer as an ordinary window.
* Draw at the **window's** size, one eye, and lay the UI out for it: undo the
  "twice as wide" canvas (`askForSize`, `arrangeCanvas`, `fitHudToCanvas`) and call
  `gViewerWindow->handleResize(...)` so the viewer believes the window is one eye wide again.
  Ignore `set_render_size` while flat -- it describes two eyes -- and follow the window: the host
  resizes the X11 window when the person resizes it on the Mac (`ClientMessage::Configure`).
* Give the pointer back: `LLWindow::sCursorMap = nullptr` (not `mapCursor`), and let the viewer
  draw its normal cursor (`drawCursor` should do nothing when flat).
* Stop the head moving the camera (`turnByHead` / `headRotation` only when `isStereo()`), and go
  back to the viewer's own field of view (`setDefaultFOV` was set from the glasses in `detect`;
  the usual default is 60 degrees).
* Draw the UI the ordinary way instead of on two depth layers (`beginUI`/`drawUI`/`drawLayer`
  and `SpatiWorldChromeDistance` are for the room).

On `set_glasses 1`, do the reverse: `sEyes = EYES_SIDE_BY_SIDE`, `tell` the three room messages
again (the same ones `detect()` ends with), re-ask for the size, re-lay-out. The simplest correct
shape is probably one function, `SpatiandStereo::setRoom(bool)`, called from `detect()` and from
`listen()`, so start-up and switching cannot disagree.

Keep `listen()` reading the socket when flat -- it is the only way back.

## How to test it

Without a Mac: the host can be told by hand. On deepmagpie, with the viewer running under the
host, nothing sends `set_glasses` unless a session says so; the Mac app does (it says
`Glasses { on: false }` when it connects, because "Show windows" is "always on this Mac" or no
glasses are plugged in). So the shortest loop is:

1. Open Spatiand on the Mac (`mac/make-app.sh`, then `open -a ~/Applications/Spatiand.app`), launch SpatiWorld from its
   menu. The host log (`journalctl --user -u spatiand-host`) shows `the session has no glasses on`
   and `told spatiworld: set_glasses 0`.
2. The viewer's log should show it acting on it. Before the change: the unknown-message warning.
3. A flat viewer in a Mac window is the pass: a sharp single-eye picture the size of the window,
   the normal arrow, clicks where you click, and resizing the window re-lays-out the UI.
4. To go back, change "Show windows" in the Mac's settings or plug the glasses in; the host
   log shows `set_glasses 1` and the viewer returns to two eyes.

The test rig in `~/swtest.sh` and `~/xclick.py` still applies for launching and clicking.
Build in the Ubuntu 22.04 root (see the jammy-build note) or the binary will not load on the Deck.

## Things to know

* The viewer's catalogue entry must stay `kind = "vr"`: only those get the control socket.
* The host does not change what the viewer is *launched* with beyond `SPATIAND_GLASSES`; the
  catalogue's `eyes = side_by_side` still applies, so the viewer must treat the glasses answer as
  stronger than `SPATIAND_EYES`.
* Sound from a VR viewer stays flat stereo on a Mac without glasses; nothing to do there.
