# Where Spatiand is

A working record: what is finished, and what is not. Kept short on the finished side — the
commit messages carry the reasoning, and `git log` is the place to look for *why* something is
the way it is. The unfinished side is deliberately detailed, because that is where the next
session starts and the thinking behind it is otherwise lost.

Last updated 2026-11-18 (the section below; the rest is as of the sidecar).

---

## 2026-11-17 and 11-18: what changed, and what was not run on hardware

Over two hundred commits in two days. **Almost all were verified by tests and by
headless snapshot rooms only.** On-device evidence exists for three things: the Mac's hardware video
decoding in SpatiMesh on Chromium 157 (measured on littleotter, docs/spatimesh-mac-port.md); what
the owner confirmed himself (the steadiness "much better", SpatiKey's entries opening, the host
list fixed). Where a commit says "not run",
the doc of its area says so, and that holds here.

| Area | What changed | Not run on real hardware |
|---|---|---|
| Steadiness (all three) | The held view, the press that goes down where the pointer rested, `steadiness` in `prefs.toml`; `SPATIAND_STEADY` now sets the press look-back and the row as well as the view; one NaN head sample no longer leaves the view NaN | Measured on a recording and in tests; the owner: "much better" |
| The pose channel in the SDK | `App::ask_poses`, `pose`, `set_frame_pose` (docs/spatiand-apps.md) | Headless room with a turning head, and the Mac's shared-memory tests; not in the glasses, not on the Beam Pro or Mac rooms (only built there) |
| Beam Pro: frames | Spatiand's own apps hand frames as hardware buffers, without a copy | Compiled (`cargo ndk check`); the commit records no run on a phone (it was off the network) |
| Beam Pro: other | Waiting screen from the Deck's code; the monitors' network feed removed | The Java of the feed's removal not compiled; the screen not seen |
| Mac | A locked Mac used from nowhere; notification setting, press and reminder; recording's microphone gate; ten-bit VideoToolbox on the GPU; Bluetooth, launcher and catalogue off the main thread; menus; the room not listing itself | Built on littleotter; **not run** against a lock, a muted banner, a Bluetooth device or a re-attaching session. The Mac's last install was at 17:08 on 2026-11-18, then it was switched off: **it needs a build for the commits after it** |
| Hosts and rooms | The room never lists itself, a re-paired host replaces its entry; hosts' notifications; menu list closing; a `.spatiapp` confined to its folder | Tests and rigs, not between two real machines (the host list: the owner confirmed it fixed) |
| SpatiKey | Entries over the socket; rekey on removing a holder; the editor, the recovery code, the window keeping up; holders on Mac and Android | Soft-service rigs and tests; **no real Touch ID or BiometricPrompt, no device with a sensor but the Mac**; the owner: entries open |
| Assistant | Never sees SpatiKey's or a locked profile's windows; a terminal's dictation is told where it is | Tested in the snapshot room |
| Clipboard | A password copied on the phone or Mac stays there; one copied in the room is put on them marked secret | Tests; not seen in a password manager or Android's preview |
| Recording | Finished on leaving, on failure and when the glasses' window goes; sound placed where it began | Tests; the Beam Pro and Mac not run |
| Video (SpatiReel) | Raw streams at their own pace, deinterlacing, episodes chained, subtitles, upright and non-square films, non-UTF-8 names | Tests, snapshot room, the Deck's VA-API in test processes; MediaCodec and VideoToolbox with a raw stream not run |
| Apps (Loupe, Quill, Shelf, Pal) | The review's fixes; non-UTF-8 names; look-and-feel rules and the SDK's new widgets | Tests and snapshots; the look-and-feel rows say what was seen |
| Removed | The Rust SpatiMesh and WPE (earlier), `mse.rs` and what only its pages used, `sp_pointer_speed`, `sp_key_evdev`, press-twice-to-delete in SpatiPal and SpatiReel, SpatiKey's own Confirm | |

**Open, and not hidden:**

* The Mac's start-up race (ANGLE's allocator shim taking over `malloc` while AppKit makes the
  glasses' window) is worked round by loading ANGLE early; the proper cure is an ANGLE built
  without the shim (docs/mac.md).
* Deinterlacing of MediaCodec and VideoToolbox frames is a shader blend, a workaround; the proper
  one is FFmpeg with Metal on the Mac. VA-API and software use FFmpeg's own filters.
* A protocol flag marking a window secret is wanted: today the assistant is kept from SpatiKey's
  and the profile locks' windows by their application ids and titles, and **can still click and
  type into a secret window**.
* **A locked Mac still receives clipboard sync**, though it takes nothing else from a session.
* SpatiKey's holders are not signed: a removed device that is still linked for sync can bury
  entries (docs/spatikey.md).
* Raw AV1 (`.obu`) cannot be seeked.
* What waits for the next Chromium build: `third_party/chromium/patches/README.md` lists them.

---

## Working

Verified on hardware unless noted.

**Display.** Owns the GPU with no desktop underneath. Glasses negotiate side-by-side by
modesetting at the native mode first (the link must be up before the MCU switch means
anything), forcing mono, then stereo. The double-width mode is matched by *shape*, not against
3840x1080 — the One Pro is 1920x1200 per eye. 72 fps sustained. Unplugging holds the session
and shows a waiting screen; replugging hands back.

**Head tracking.** Ported from HoloFrame. The axis map is measured, not guessed: gravity in
three poses gives up = sensor +Z, left = +X, forward = −Y. That is `AxisMap::XREAL_AIR` and it
is the default. Calibration exists in-world (never a terminal) with B to cancel; the HUD can
cycle the four valid pitch/roll interpretations, since a wrong one is a valid rotation and
nothing in the measurement can catch it. **The room is held still while the head only sways**
(2026-11-17; the owner: "a small movement and the screen shakes with the shake of my body"), on
every platform -- the Deck, the Beam Pro and the Mac -- at the wearer's **Steadiness**: a row in
the settings that steps off, light, medium (the default), strong (`steadiness` in `prefs.toml`;
`SPATIAND_STEADY=0|light|medium|strong` overrides it, to compare). `spatiand_track::Steady`,
applied to the orientation each frame draws with, writes to clients' pose channel, places
head-locked windows by and hit-tests the notification card with:

* **a band** (0.5, 1.2 and 2.5 degrees) within which the head moves nothing at all. The view
  moves only by as much as the head has gone past it, so a sway inside it is held to the last bit
  and nothing creeps afterwards; a sway a little wider pushes it once and then sways inside it;
* **a turn closes the band** (pushing it at 15 to 40 degrees a second and faster, at medium) to a
  fifth of a degree within its first few degrees, so a turn ends where the head is, and the band
  opens again round the head as it slows -- a sway is too slow to close it;
* the view drawn **settles on the held one** in about 40 ms, and at once in a turn.

Measured (`cargo run -p spatiand-track --example steadiness [imu.bin]`, 72 frames a second): a
made-up body swaying 1 degree each way (half up and down, a third leaning) at 0.2 to 3 Hz with a
tremor and the sensors' noise moves the medium view 0.006 degrees a second at most, against 2 to
13 raw; strong holds 2 degrees. A deliberate turn of 20 to 90 degrees at 50 to 200 a second: the
view starts 54 to 100 ms after the head at medium (80 to 150 at strong, 36 to 64 at light) --
the delay XREAL's own room has -- is nine tenths of the way 4 to 16 ms after it (strong 6 to 23),
never past the end, and still once the head is. A press that nods the head 0.5 to 1.5 degrees:
medium moves nothing for a degree, 0.23 for 1.5. The Deck's recorded head (8 minutes, 2026-09-19):
in the third of its seconds when it stayed within a quarter of a degree of where it was, the
view moved 0.028 degrees a second at medium against 0.72 raw; within half a degree, 0.13 against
1.25; within a degree, 0.49 against 1.96. What the band costs: a look of less than the band moves
the room not at all, and a small one by that much less. Degrees and seconds throughout, so the
Deck's 72 frames, the others' 60 and the Deck's loop turning every few milliseconds between
frames are the same to it (its tests run at 60, 72, 90 and 260); the sensors are read at about
1000 Hz on all three (hidraw, Android's usbfs, IOKit).

**Controller.** Raw hidraw, every button confirmed by pressing it. Both pads point; right pad
click is left mouse, left pad right mouse, A/B/X duplicate them. The left pad scrolls up and down
unless it has clearly gone sideways (`spatiand-input/src/scroll.rs`, with a real thumb's swipes in
its tests). A thumb only wandering -- the pad's noise, its pulse -- moves neither the pointer nor
the page: each axis of each pad is held until the thumb has gone 0.004 of a pad from it, 0.008
once it has rested 100 ms, and then followed that far behind, so a slow scroll never shakes back
(`spatiand-input/src/steady.rs`; the room's reading only, a game's layout gets the pads as they
are; the Mac's and the Beam Pro's pads and phone pointer too). **A pointer stays where its
button went down**, whatever aims it, on every platform: every pointer is cast through the head,
so a press under a head that is never still was a few pixels of movement with the button down --
to a browser a link dragged, not clicked -- and a second click never landed on the first. The
room holds it there until the aim has turned 0.4 of a degree away (about 18 pixels of a window at
its default distance), then lets it go and makes up what was held back over the next of its
travel, so a drag or a selection begins where the button went down and nothing jumps; and for
500 ms from the press it stays for the next press, so a double and a triple click are on one
point. A mouse or a trackpad is let go after 0.1 of a degree of its own movement, the head under
it still having the 0.4. The room's pointer is drawn where it is held
(`crates/spatiand/src/press_hold.rs`, used in `PointerState::motion`, which every pointer passes;
`pointer_hold_degrees` and `pointer_hold_mouse_degrees` in `prefs.toml`, 0 for off; the figures
are chosen from the sizes around them, not yet from a recorded head). **And a press goes down
where the pointer rested just before pressing moved it** (2026-11-17, with the steadiness): a
thumb rolls as it pushes a pad's click, a tap tips the phone, the head nods with the effort, all
before the button is felt. So a press looks back (80, 120, 150 ms at light, medium, strong) for
the last frame the pointer was at rest, and if it has moved less than 0.6, 1 or 1.5 degrees
since, the application is told the pointer is back there and the button goes down there; the
hold then allows for that jolt. Not for a pointer that came to rest where it is pressed, one
moving all that while, a mouse, or a layout's own mouse. With the room's view: a press that nods
the head up to 1.5 degrees clicks within a hundredth of a degree of where the wearer pointed at
medium and strong, and the pointer does not move while the button is down (`press_hold.rs`'s
tests; off, it missed by the whole nod and dragged). Held, a slow wobbling scroll went
stop and start, so each of its steps goes out over the frames after it, 0.35 of what is owed a
frame (`ScrollSpread`, about 40 ms on average): from the owner's recorded thumbs, frames sending
nothing mid-scroll fell from 28% to 10% (`spatiand-input/examples/replay_scroll.rs`). Both pads *pressed* together
(resting both thumbs is nothing) are a gesture of the window under the pointer, which stays
put: the thumbs swept sideways are back or forward (a mouse's back and forward buttons), spread
or drawn together a pinch (the standard Wayland pinch, which Chromium zooms the page by; a
Spatiand app that zooms itself gets its own). So a pad's click waits 80 ms for the other's,
except with the on-screen keyboard up, where both thumbs type (`spatiand_input::press`).
Haptics work — `ID_TRIGGER_HAPTIC_PULSE` (0x8f), pad byte 0 is right, 1 is left.

**World.** 360 environment (generated studio, plus five NOIRLab observatory panoramas), glass
bubble launcher with real Breeze icons and freedesktop categories, STEAM HUD, status bar with
clock and battery, on-screen keyboard, screenshots to `~/Pictures/Screenshots`.

**Windows.** Real Wayland clients as quads on a sphere, facing the viewer in yaw and pitch.
Title bar drag to move, left thumb for depth, two-thumb gesture to move and scale, D-pad to
cycle focus. Chromium-family apps get `--ozone-platform=wayland`, and Flatpak `@@u … @@` markers
are stripped. A window's subsurfaces are drawn and pointed at as its menus are (Chromium's bubbles
are subsurfaces: the profile menu, the address bar's suggestions, a login field's list), except
through another window in front of them.

**Sidecar.** Second `DrmCompositor` on the Deck's panel: CPU/GPU/memory graphs, clock, battery,
volume and brightness. Flip completions are tracked per-CRTC — a single flag lets one screen
consume the other's and that screen then never presents again, silently. The volume and
brightness bars are touchable (below).

**Sound devices in the glasses.** The settings' *Sound devices* row, which the Mac has had, is
the Deck's too (2026-11-17): where sound comes out and which microphone listens, from PipeWire
through the mixer's thread (`volume_keys::Mixer`, `wpctl`, the sidecar picker's lists), chosen
with A; the tick moves at once and the list is read again every two seconds while it is open.
Offered where there is a `wpctl`. `SPATIAND_VIEW=audio` photographs it.

**Notifications.** The room answers `org.freedesktop.Notifications` on the session bus
(`crates/spatiand/src/notifications.rs`, D-Bus written out by hand, no stack): a desktop
application's notification is shown on the card under the status bar, as the Mac's banners and
the Beam Pro's are, and pressing it is its default action. The name is asked for with
`ALLOW_REPLACEMENT | DO_NOT_QUEUE`, so Plasma's shell, where it runs, keeps it; in the room's own
session it is free. `SPATIAND_NOTIFICATIONS=seconds` in the snapshot waits for one to arrive.

**Tooling.** `SPATIAND_BACKEND=snapshot` renders a frame to a PNG from a render node, with no
display, session or headset, and can host a real Wayland client while doing it. Almost every
layout bug in this project was found by looking at its output. `tools/probe-*.py` established
the controller map, the haptic command and the IMU axes.

294 tests. Build on the Deck in the `holo` distrobox; the Mac has no Linux graphics stack.

---

**Sidecar touch.** The panel's digitiser, read through evdev — `spatiand_input::touch`. Found
by capability (`INPUT_PROP_DIRECT` plus the MT slot axes) rather than by name, so it is not a
Deck quirk. Protocol B decoded into contacts, with nothing acted on before `SYN_REPORT`.
Opened through libseat, because the event node is `root:input` with no ACL and only logind can
hand it over. Volume and brightness are real sliders; the bars went from 30 px to 56 px
because a landscape pixel here is 0.118 mm and a fingertip is 8–10 mm.

**Window resize.** The frame is a grab target — left, right, bottom and both bottom corners,
each with a turned double-arrow cursor. Pixels follow the world size at constant density, so
the client gets more buffer rather than a bigger picture of the same buffer: text keeps its
angular size and more of it fits. The opposite edge stays put.

294 → 337 tests.

---

## Not tested by a human yet

Everything above has tests and builds; these two have never had a finger or a thumb on them.

- **Sidecar touch.** Discovery, opening and the `EVIOCGABS` ranges are confirmed on the real
  panel (`cargo run --example touch-probe`, as root). What is not confirmed is the quarter
  turn, and the libseat open inside a live session. The sidecar draws a dot under every
  contact, which is the fastest way to tell the two apart: **no dot at all** means the device
  never opened; **a dot that mirrors the finger along one axis** means the turn in
  `Sidecar::touch_to_layout` has a sign wrong. Those are different bugs and they look
  identical from a description.
- **Window resize.** The frame renders at the right thickness (checked with the snapshot
  renderer) and the arithmetic is tested, but no edge has been dragged.

### Games

Gamescope. The rest of what this section used to promise is built: see
`docs/input-mapper.md`. Every control is remappable per application in a Steam-style editor,
the Deck, any Bluetooth pad and the glasses' gyro and temple buttons all feed it, and the game
sees one virtual Xbox pad and nothing else. Steam runs alongside with `-nojoy`, which is what
lets a Steam game be launched at all while Spatiand owns the controller.

Not yet played: a real game, on the glasses, with a person holding the Deck. Everything up to
that is checked — the layouts and the engine by 47 tests, the pad by `--ignored` against the
kernel and by asking SDL what it can see, the editor by `SPATIAND_VIEW=controller`, and the
hiding of the physical pads from Proton by rendering what Wine enumerated.

The first attempt to play one found three separate faults, none of them in the mapper: an X11
window was never given X's own input focus, so a Wine game believed it was in the background
and ignored the pad and the keyboard both; a window bigger than the X screen had a corner its
pointer could not reach; and a window's audio sink was closed by the first window of its app to
disappear, which for a game launched through Steam is the window Steam puts up while it
prepares the game. All three are fixed and measured — see [x11.md](x11.md) and the list
below — but the game itself still has to be played.

### Smaller, and genuinely optional

- **Stereo per window** (`spatiand_stereo_v1`): the shader already takes a UV range, so an SBS
  source needs the protocol and a per-window layout, not new rendering.
- **Media player**: `spatiand-media` on libmpv. `Scene::set_sky` is the seam — playing a 360
  video is "swap the sky texture each frame and put the projection back afterwards".
- **`EvdevGeneric` / `NullHmd`**: the plan wanted a second backend early to prove the traits are
  not Deck-shaped. `NullHmd` exists; the generic input backend does not.
- **Curved window panels**, window persistence across sessions, per-game input profiles.

---

## Things that were true and cost time

Worth keeping, because each was invisible from the outside and none would be guessed twice.

- Smithay delivers `event.location - loc` to a client. Passing the surface-local position as
  both makes every delivered position (0, 0): motion appears to work and nothing is ever under
  the cursor.
- `Space::elements()` reorders when a window is raised, so an index into it is not an identity.
- A client texture arrives with GL's default `NEAREST_MIPMAP_LINEAR` filter. With no mipmaps
  that texture is *incomplete*, and an incomplete texture samples as opaque black — with no
  error anywhere.
- `XrealGlasses::drop` stops the IMU stream, and `hmd = open_any()` evaluates the new handle
  before dropping the old. Opening twice leaves a device that looks healthy and never sends.
- The glasses swap EDID when entering side-by-side. Nothing re-probes the connector unless
  something asks, and with no desktop running there is nobody to ask.
- MCU length fields count themselves. The controller's do not.
- One degree is about 2% of a window's height at 2.2 m. Anything sized as a fraction of
  something else needs checking in degrees before it is called a target.
- An X11 window's focus is two things: the Wayland surface XWayland draws it into, and X's own
  input focus. Giving the seat the surface does the first and silently skips the second, and
  the symptom is not "no keyboard" but "this game ignores its gamepad" — Wine decides it is in
  the background. `xdpyinfo | grep focus` is the whole diagnosis: `PointerRoot` means nobody
  set it.
- A sink belongs to a process, not to a window, and the two do not end together. Closing it
  when the first window goes takes the sound away from an app that opens a splash and then a
  window — which is every game launched through Steam.
- One sink, several windows, and the aim has to pick one of them. Size alone picked Steam's
  window over the game's, because they are the same size and the tie went to the older — so a
  game straight ahead sounded from wherever Steam was standing. Between windows that look
  alike it is now the focused one; size still wins when one window is much bigger, which is
  the case that rule was written for.
- A game mixes for the device it is shown. Offered a 7.1.4 sink, Stumble Guys mixed for twelve
  speakers and what arrived had its front-right channel silent and its music somewhere between
  the centre and the LFE — recorded off the sink's own monitor, so that was the game's own mix
  before anything here touched it. A window's sink is now as wide as the application can use
  well: two channels for a game, the full layout for something playing a mix it did not make.
- Steam tells every game it launches which controllers Steam Input handles — several hundred
  ids, every real pad among them — and Proton believes it. A virtual pad wearing a real
  controller's identity is therefore invisible to the game; wearing Steam's own virtual gamepad
  identity, `winedevice.exe` opens it within seconds. Same device, same buttons, different name.
- A PipeWire buffer is as big as the largest quantum it was negotiated against, not as big as
  the quantum in force. `Buffer::requested()` is how many frames the graph actually wants this
  cycle, and filling the buffer instead drains the queue faster than the app fills it.
- `/proc/<pid>/environ` is *empty*, not missing, for a process that has been started and has
  not finished starting. Anything that identifies a process by its environment has to be able
  to wait.
- Steam's container shares this machine's process numbering (one `NSpid`, the initial
  namespace) and passes `PULSE_SINK` and `PIPEWIRE_PROPS` through untouched. Both were worth
  measuring before believing: they are what ties a game's window and its sound back to us.
- **Build in the `holo` distrobox, not `spatiand`.** Both were made from `archlinux:latest`,
  but at different times, and that tag moves: `holo` has glibc 2.41, `spatiand` has 2.44, and
  SteamOS has 2.41. A binary from the newer box builds and links perfectly and then dies at
  the first instruction with `GLIBC_2.43 not found`. Because spatial mode replaces the
  session, "dies before printing anything" looks like a black screen that never comes back —
  not like a build error. `tools/setup-buildbox.sh` now checks the two glibcs instead of
  printing them, and `go-to-spatiand.sh` runs `ldd` before switching sessions, so the failure
  arrives as a dialog rather than as a dark room.
- **The glibc floor is set by the build container, not by anything in this repo.** The only
  symbols above 2.35 are `pidfd_spawnp` and `pidfd_getpid` at 2.39, which Rust's `std` uses
  when the glibc it was *built against* offers them and silently does without when it does
  not. So building in an older box lowers the floor with no code change.
- **`podman unshare` is needed to clean `target/`.** The other box ran with a different user
  namespace, so some of `target/` ended up owned by uid 100000 and `deck` could not write to
  it — cargo reports that as a bare `Permission denied` on a fingerprint file. `rm -rf` will
  not shift it either; `podman unshare rm -rf target` will.
- Rotating a basis by reusing the axis you just rotated is not a rotation, it is a skew. Both
  new axes have to come from the old pair.
