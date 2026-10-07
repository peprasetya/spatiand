# OpenXR and Spatiand

Spatiand has an OpenXR runtime of its own, `crates/spatiand-openxr`. An OpenXR application —
a game, a viewer, a tool — finds it through the ordinary OpenXR loader, and its two eye views
become the room: on the Deck, in the glasses; from another computer, streamed to a headset;
on a Mac or a Beam Pro as the remote client.

**It is not a conformant runtime and does not say it is.** Conformance is a formal process with a
test suite and an Adopter Agreement; none of that has happened. What follows is what it does, what it
does not, and how each was checked.

## How it works

```text
 the game ─── OpenXR loader ─── libspatiand_openxr.so ───┐   in the game's own process
                                                          │
          pose channel (shared memory)  ◄─────────────────┤   spatiand_xr_v1, over Wayland
          side-by-side dmabuf + set_layer(projection)  ───┤
          set_frame_pose, render_size                      │
                                                          ▼
                       Spatiand on the Deck   — or —   spatiand-host, which streams it
```

The runtime is a **Wayland client**. It asks the compositor named by `WAYLAND_DISPLAY` for the pose
channel (`spatiand_xr_v1`, defined in `crates/spatiand-proto/protocol/spatiand-xr-v1.xml`), makes
a window, declares it a side-by-side `projection` layer, and at every `xrEndFrame` copies the two
eyes into one image whose memory is exported as a dmabuf and attached to that window. Nothing is
copied through the CPU.

The point of putting a protocol in the middle is that **the same library serves both places**. On
the Deck the compositor is Spatiand; on a machine that streams to a headset it is `spatiand-host`,
which speaks the same protocol (`crates/spatiand-host/src/xr.rs`) and hands out the head the
headset last sent. The game cannot tell which it is talking to. The host then streams the window
like any other, the headset sees a window whose layer is `projection`, and shows it as the room.

### What each piece is

| Piece | Where |
|---|---|
| Loader entry and dispatch | `src/lib.rs` — `xrNegotiateLoaderRuntimeInterface`, `xrGetInstanceProcAddr` |
| The OpenXR calls | `src/api.rs` |
| Handles and state | `src/object.rs` |
| Head and eye poses from the ring, interpolated to the time asked | `src/pose.rs` |
| The Wayland connection, window, dmabuf buffers | `src/wl.rs` |
| Vulkan: the application's device, the copy and quad pass | `src/vk.rs`, `shaders/` |
| Turning a frame into a picture | `src/present.rs` |
| OpenGL support | `src/gl.rs` |
| Controllers | `src/input.rs` |

### Graphics APIs

**Vulkan** (`XR_KHR_vulkan_enable2`, and `XR_KHR_vulkan_enable`, which is what Proton's `wineopenxr`
uses). The runtime creates the application's instance and device, adding external-memory extensions.
Swapchain images are allocated in that device; at `xrEndFrame` they are blitted into the frame.

**OpenGL** (`XR_KHR_opengl_enable`, Xlib/GLX or Wayland binding). The application gets ordinary
textures — any format, any array size. When it releases an image, each layer is copied on the GPU
into a *linear* image allocated by Vulkan and imported into GL (`GL_EXT_memory_object_fd`), and Vulkan
reads that. Sharing the application's own tiled texture was tried first and came out as confetti —
the right colours in the wrong places — because two drivers do not mean the same thing by "optimal"
tiling; the linear staging image is why that does not matter. The GL driver and the Vulkan device are
matched by UUID.

### Layers

A **projection layer** is the room. A **quad layer** is drawn over it, in the pose and size the
application gave, with a small Vulkan pipeline (premultiplied or unpremultiplied alpha, either eye
or both). Cylinder, equirect and depth layers are not composed yet; the log says so once.

### The picture stays put

The compositor is told, at every commit, which head orientation the picture was drawn for
(`set_frame_pose`), and turns it by how far the head has moved since. Without it a picture drawn
for one head and shown to another swims against the world. Rotation only; translation is not
corrected.

Over a network the same thing is done in three more steps, because the headset and the game do
not share a clock: the slot the game reads its head from carries a number (on a host, which viewport
the headset sent), the game hands it back with `set_frame_pose`, the host stamps it into the picture's
header, and the headset looks up the head it sent under that number. On the Deck every picture shown from
the test host was matched to its head this way (the session log says how many: `(104 for a head it
knew)`). The Deck, the Beam Pro (same code) and the Mac all do it. Rotation only, in all of them.

### Controllers and the pointer

Nothing tracks a hand. Two things stand in for a pair of Touch controllers:

* **The gamepad** that is already there -- Steam's virtual gamepad on the Deck, `Spatiand Gamepad` on a
  host -- gives the sticks, triggers, bumpers and face buttons, by Touch's layout. An application is told
  it has the best of Touch, Index, Windows motion, Vive or the simple controller that it bound. Haptics
  are accepted and ignored.
* **The wearer's pointer** gives the hands' *aim*. A game that wants a laser from a hand draws it itself,
  from the pose of the hand it is given -- and the compositor's own reticle would be a second pointer, at
  the wrong depth. So the runtime declares its picture `set_cursor_drawn` (nothing is drawn over it by
  Spatiand), takes the pointer's motion as ordinary `wl_pointer` events on its window, and turns both
  hands to point **at the place the pointer is on**: the ray from the left eye through that pixel of the
  picture, ended on the nearest quad layer it crosses (the game's menu) or four metres out. The laser
  the game draws from each hand then ends exactly where the wearer's pointer is. The pointer's left button
  pulls the right hand's trigger and its right button the left's, so a menu is used with the pointer the
  wearer already has -- the same arrangement as SpatiWorld's, with the game doing the drawing.
  This works the same over a network: the headset sends its pointer to the host as it does for any
  window, and the host's seat hands it to the runtime.

* **The gyro** can aim the right hand instead, for a game that needs a hand to point -- a gun, a laser, a
  grab. In the controller layout, set the gyro's output to **VR right hand** (the *VR game* template does,
  with the gyro on while a thumb rests on the right trackpad). The hand then points where the head points,
  turned by as far as the gyro has turned since it came on (sensitivity, deadzone, which rotation is left
  and right, inversion and what turns it on are the gyro's own settings, as for any gyro output), and eases
  back to the head when it goes off. It rides the pad's first two spare axes (`Report::extra`, -1..1 for
  -90..90 degrees, right and up), so it goes anywhere the pad does: the Deck's own, a host's across the
  network, a Mac's or Android's DualShock 4 through the same layouts. It wins over the pointer for that hand.

The pointer holds the hands only while it is in use (it moved or clicked in the last four seconds); at rest
each hand sits in front of the head, pointing where the head points, so a game that moves relative to a hand
is not sent wherever the pointer last stopped.

## Running something

Install the runtime for this user:

```bash
cargo build --release -p spatiand-openxr
tools/install-openxr.sh            # --default makes it this user's active OpenXR runtime
```

That copies the library and a manifest to `~/.local/share/spatiand-openxr/` and points the
session's environment (`~/.config/spatiand/session.env`) at it, so anything started from inside a
Spatiand session finds it. For one command in a terminal:
`XR_RUNTIME_JSON=~/.local/share/spatiand-openxr/spatiand_openxr.json <game>`.

### A Windows game under Proton (X8, tested)

Proton 10 and later run a Windows game's OpenXR through `wineopenxr`, onto a *native* Linux
runtime — which is this one. One thing in the way: `wineopenxr` will not start unless the game's
prefix says `HKCU\Software\Wine\VR` has `state = 1`, and Proton writes that only when SteamVR is
installed. Without it the game logs `XR_ERROR_RUNTIME_UNAVAILABLE` and runs flat, and the runtime is
never even loaded. So, once per game, with the game closed:

```bash
tools/xr-seed-prefix.sh <steam app id>      # X8 is 1763510
```

Then start the game from inside Spatiand (so it inherits `XR_RUNTIME_JSON`). X8 is Unreal Engine 4
using the Oculus plugin's OpenXR backend over D3D11; it creates a 3840×1080 swapchain, a depth
swapchain and a 4 m world-space quad for its interface, and all three are composed.

What X8 binds (the runtime logs every suggested binding at debug level, `SPATIAND_OPENXR_STDERR=1` or the
log file): on Touch, `move_x`/`move_y` are the **left** thumbstick and `turn_x`/`turn_y` the **right**; a
mirrored `inverted_*` set swaps the sticks for a left-handed player; `move_z`, `use_*` and `ability_*` are
the triggers; `sprint` and `crouch` are the stick clicks; `togglemenu` is the left menu button (Start) or Y.
So under X8 the gamepad's sticks are not "look and move" as on a screen: the head looks, the right stick
turns the body, and the triggers also feed `move_z`.

A game that draws the room often also opens an ordinary window of its own (Unreal's "X8 (Shipping)"
mirrors what the headset shows). It is not something to look at -- the room is that picture -- so a window
that belongs to the same process as a picture drawn through the runtime is put away as it appears, and is
left out of the window list. It keeps running. (The process is the Wayland connection's, and for an X11
window the `_NET_WM_PID` it set.)

### Glasses are not headsets

A headset has 100 degrees or more across; XREAL's glasses have about 40, and a game lays out its menus and
tutorial for the first -- what it puts 30 degrees below the middle is not in the second. A wider field was
tried (telling the game a wider view and turning its head by the same factor) and **removed**: the picture
reaches the compositor as a rectangle drawn for the glasses' own field, so a wider one left black round it,
and nothing made the game's panels any more reachable. A game is shown the glasses' own field. The runtime
logs, at debug level, where each quad layer is relative to the head (and the head's height and pitch), which
is how to tell a panel that is out of the field from one that is merely somewhere else.

A panel that is larger than the glasses' field is therefore **fitted**: shown smaller, in front of the head
and wholly in view, at the distance the game put it, and the wearer's pointer is mapped back to the same
place on the game's own panel (so the game's hand points at the button the pointer is on). A panel that
already fits stays where the game put it. The runtime logs, per panel, how much of its size is shown. The
game's own laser still ends where the game thinks the panel is; the hover and the click land on the right
button.

### Which control does what

At debug level the runtime says, as they change, what the pad itself reads (`pad: left stick ... triggers`)
and what the game is told for each action (`input: move_x (left) = Float(0.8)`), so a stick that does the
wrong thing can be followed from the hardware, through the binding, to the action the game reads:
`SPATIAND_OPENXR_STDERR=1`, or `~/.local/share/spatiand-openxr.log`.

### Conformance

Khronos' conformance suite ([OpenXR-CTS](https://github.com/KhronosGroup/OpenXR-CTS), Apache-2.0) is the
measure of "does this follow the standard". Conformance is **not claimed**: the suite's interactive tests
(a person in the headset) were not run, and only the Vulkan graphics plugin was. What was run, on the Deck,
against a headless Spatiand (`SPATIAND_BACKEND=headless SPATIAND_HMD=null SPATIAND_INPUT=off`, with an
empty `XDG_CONFIG_HOME` so it does not connect to a remote host):

* the non-interactive tests: 103 test cases listed, of which 64 pass and 39 are skipped because they test
  extensions this runtime does not have; none fail;
* what was checked and fixed on the way is in the code as validation: swapchain call order, frame loop
  order and times, layer validity, action and action set names, attaching, syncing and querying,
  interaction profiles and their component paths (the table is `profiles_table.rs`, generated by
  `tools/gen-profile-table.py` from the suite's own data), path format, result and structure names,
  `xrLocateSpaces`, and version gating of entry points.

To run it: build the suite in the `holo` container (`cmake -DPRESENTATION_BACKEND=xlib`, with git-lfs, Vulkan
headers, glslang and the X11 development libraries -- it has no Wayland presentation backend), start the
headless Spatiand, and `conformance_cli -G Vulkan2 "~[interactive]"` with `XR_RUNTIME_JSON`,
`WAYLAND_DISPLAY` and `DISPLAY` set. A test that crashes the runtime ends the whole run, so run test
cases one at a time (`--list-tests`) to find which.

### From another computer

Add the game to the host's catalogue as a `kind = "vr"` application and make the runtime visible to
it. The host needs the OpenXR loader (`libopenxr_loader.so.1`) and the runtime library; set
`XR_RUNTIME_JSON` in the catalogue entry's `env`. The headset needs nothing new: it sends its head
and eyes (`Viewport`, which the Deck, the Beam Pro and the Mac all do) and shows a `projection`
window as the room.

The host tells the runtime what size an eye should be drawn at (the pose channel's `render_size`,
from the headset's own viewport) so a game draws what the headset wants rather than what a desktop
would; `SPATIAND_OPENXR_EYE=WxH` overrides it.

## What is not there

* **Conformance.** Not claimed, not tested with the Khronos CTS.
* **Hands.** A gamepad and a pointer standing in; no tracking (a gyro could aim a hand, and nothing does yet), no hand-joint or controller-model extensions.
* **Reprojection of translation**, anywhere: a few centimetres of head movement in a few milliseconds is not corrected.
* **Layers:** cylinder, equirect, cube, depth-tested composition. Only the first projection layer.
* **Multisampled swapchains** (`maxSwapchainSampleCount` is 1: an application resolves its own).
* **Passthrough/AR blend modes, visibility masks, spatial anchors, eye tracking, haptics.**
* **Android or Mac as the OpenXR host.** They are clients only; the runtime runs where the game runs.
* **A game that talks OpenVR rather than OpenXR.** Nothing here helps.

## Debugging

The runtime writes `~/.local/share/spatiand-openxr.log` (or `SPATIAND_OPENXR_LOG`); the home
directory because a game's container gives it a private `/tmp` and `XDG_RUNTIME_DIR` where a log would
never be found. `SPATIAND_OPENXR_STDERR=1` mirrors it to standard error. It says what the application
asked for, which swapchains it made, which layers it submitted, and anything it asked for that is not
implemented.

Without glasses, the snapshot backend draws projection layers too:

```bash
SPATIAND_BACKEND=snapshot SPATIAND_CLIENT=/path/to/openxr-app SPATIAND_SNAPSHOT=/tmp/x.png \
    SPATIAND_HMD=null SPATIAND_INPUT=off spatiand
```

and `SPATIAND_SNAPSHOT_YAW=12` turns the head, which is how reprojection is checked. The test
clients used so far are the `openxr` crate's Vulkan example, a copy of it that adds a quad layer,
and a small GLX client with a white marker at the top left and a red one at the top right — which is
how it was found the OpenGL picture was the right way up.

### Things that cost time

* **Proton's `wineopenxr` reports `state = -1` and nothing says why.** See above; the cause is in
  `vrclient_main.c` of Valve's Proton.
* **A container's `/tmp` is not yours.** Logs, sockets and manifests belong under the home directory.
* **A running Steam ignores a new environment.** `steam -shutdown` and start it again.
* **`pkill -f` over ssh kills the ssh.** The command line it matches contains the pattern.
* **Tiled textures do not cross GL↔Vulkan.** See OpenGL above.
* **naga's GLSL front end** wants `layout(binding = n)` without `set`, and a separate texture and
  sampler.

## Why not Monado

The first version of this document argued for Monado and called writing a runtime a large project to
start only with a reason. The reason turned out to be the remote case: the same library has to run
beside a game on a machine with a GPU and no display, take its head from a network headset, and put
its pictures into a stream. That is a Wayland client with a pose channel, and it is small. Monado
remains the right answer for conformance, and this is not trying to be that.
