# Spatiand

A 3D spatial desktop: windows hung on a sphere around you, seen through XREAL
glasses. It runs on three systems: the Steam Deck (a third mode alongside
Desktop and Game), the XREAL Beam Pro (Android) and a Mac.
[spatiand.com](https://spatiand.com/) shows it.

Windows stay where you left them when you turn your head. Each one's sound
comes from where the window is. The Deck's own screen becomes a control panel
for the session you are wearing. Applications running on another computer
arrive as windows of their own, beside the local ones.

**This is early.** It runs, it is used, and it is not finished. What follows is
an honest account of which parts do what. Most of the work of 2026-11-17 and
11-18 was checked by tests and in headless rooms, not in the glasses;
[docs/state.md](docs/state.md) says which.

**Three systems, one core.** The room began on the Steam Deck and is the same
code on the Beam Pro and on the Mac (and on other Linux): a port adds only what
the system itself needs (its libc, its windowing, its GPU buffers, its sound),
never another core. Most of this page still speaks of the Deck, where it is
most worn-in; [docs/beam-pro.md](docs/beam-pro.md) and [docs/mac.md](docs/mac.md)
say where the other two stand.

**Its own applications**, each a Wayland client of the room on every system,
speaking `spatiand_xr_v1` (the room's extensions to Wayland) and sharing one SDK
and one set of rules for look and words
([docs/spatiand-apps.md](docs/spatiand-apps.md),
[docs/look-and-feel.md](docs/look-and-feel.md)):

  * **SpatiMesh**, the browser: Chromium's Linux browser with Spatiand's
    patches, the same on the Deck, the Beam Pro (built for Android's libc) and
    the Mac (Chromium's Linux user interface on macOS), moved forward to the
    newest stable Chromium every month. [docs/spatimesh.md](docs/spatimesh.md);
    `third_party/chromium` is how it is built.
  * **SpatiShelf** (files, and remote storage through a service built on
    rclone), **SpatiQuill** (a terminal), **SpatiReel** (films), **SpatiLoupe**
    (pictures, stereo photographs, PDFs, 3D objects).
  * **SpatiPal**, the assistant, with a figure in the room, a voice and hands on
    its windows; and **dictation** into the focused text field.
    [docs/ai-integrations.md](docs/ai-integrations.md).
  * **SpatiKey**, passwords, passkeys and two-factor codes, and the browsers'
    way to them. [docs/spatikey.md](docs/spatikey.md).

**Applications from another computer.** `spatiand-host` runs on a Linux machine
or a Mac and hands each application to the glasses as a window of its own, not
as a remote desktop. [docs/remote.md](docs/remote.md).

**An OpenXR runtime of its own**, so that OpenXR games and viewers run in the
room, here or from a host. [docs/openxr.md](docs/openxr.md).

---

## What works

**The desktop.** Stereo at 3840×1080@72 through the glasses, head-tracked, with
real Wayland clients as windows you can point at, move, resize, hide and close.
Windows curve round you, the nearer the more; a hidden one keeps running and is
brought back from the window list. A 360° environment behind them. A launcher, a HUD, and a virtual keyboard.

**The room holds still.** The view is held while the head only sways, and a press goes down where
the pointer rested before the press moved it, at a Steadiness the wearer sets in the settings (off,
light, medium by default, strong), the same on the Deck, the Beam Pro and the Mac. The apps follow
one set of rules for their look and words, [docs/look-and-feel.md](docs/look-and-feel.md). Most of
these changes, those of 2026-11-17 and 11-18, were checked by tests and in headless rooms only;
[docs/state.md](docs/state.md) says what was and was not run on hardware.

**Picture in picture.** Any window can be pinned to the glass with the pin on its
title bar, or from the window list (left pins, right lets go): it then stays in
one corner of your view, tilting with your head, with a hairline frame and its
buttons showing only while you point at it. Its sound goes with it, whole 7.1.4
bed included. The corner and one of two sizes are set in the settings and
remembered; whether a window is pinned is not — a pin is temporary. Wayland has
no way for an application to ask for this, so a browser's own picture-in-picture
window is recognised by its title (in the languages the browsers ship in), and
an X11 application that asks to be kept above the others is pinned too.

**X11 applications run**, through XWayland — VLC, Kodi, Firefox — which matters
because a great many programs have no Wayland support and never will. They are
**not supported**: X11's model of menus and screen positions does not survive
being put in a room, every X11 window says so on its title bar, and
[docs/x11.md](docs/x11.md) says exactly what breaks.

**3D applications.** `spatiand_xr_v1` lets an application say how its two eye
views are packed into one buffer, whether it follows the view, and whether it is
the room itself — a 180° or 360° picture, mono or stereo, or its own two eye
views filling the view, drawn behind every window. It also hands over head and eye
poses through shared memory, so an application drawing its own views can read
them at the last moment before it draws. A surface can also ask the compositor
to fade it out while nobody is attending to it and bring it back on a glance,
which is a thing a client cannot do for itself. An application that ignores all
of it is an ordinary window and stays one. See [docs/apps.md](docs/apps.md).

**GPU buffers.** Clients can hand over decoded frames as dmabuf rather than
copying them through shared memory — which is what a hardware video decoder
produces natively, and the difference between a video player that works and one
that spends its budget on memcpy. The global carries the render node's identity,
which is also how a client's EGL finds the GPU at all.

**Spatial audio.** Each window gets its own audio sink; its channels are placed
around you and rendered to two ears through a measured head-related transfer
function. Turn towards a window and its sound turns with you. Mono through
7.1.4 keeps its shape: the front channels sit on the window's own edges wherever
you put it, the surrounds swing round behind you. Windows that make a sound grow
a speaker on their title bar.

**No fullscreen, and applications are told so.** The one output a client can see
reports the size of a *window*, so a player sizes itself for what it was
actually given. Asking for fullscreen is granted at that size, which is what
makes Kodi drop its chrome and fill the window instead of laying out for a
framebuffer twice as wide as the world.

**Input.** The Deck's trackpads are two pointers with laser beams. The D-pad
types arrow keys into the focused window, A is enter, B is escape; in
Spatiand's own menus a held direction keeps moving, as a held arrow key does. A
window switcher in the HUD brings any window to the centre of your view. A USB
or Bluetooth keyboard types; a mouse is a third pointer that fades when idle.

**Copy and paste** work between Wayland and X11 applications, and between the
Deck and a remote host in both directions. Tested end to end across seven
applications on two machines — every copy pasted into every other.

**Games.** Every control, the four back paddles included, is remappable per
application in Settings → Controller layout, and what a game sees is one
gamepad wearing Steam's own virtual gamepad identity, so Proton games launched
through Steam pick it up. A Steam game starts on a plain gamepad layout,
anything else on the desktop one. A game gets a stereo sink placed on its
window. See [docs/input-mapper.md](docs/input-mapper.md).

**Applications on another computer.** `spatiand-host` runs on any ordinary
Linux machine and hands each application to the glasses as a window of its own,
not as a remote desktop. Pictures are encoded on that machine's GPU and decoded
on the Deck's (VA-API, HEVC or H.264), and a window that is not changing sends
nothing. Sound comes from the window, as a local application's does; keys, both
pointers, the microphone and the controller go the other way — the controller
as a gamepad of the host's own, played through the application's own layout.
Applications keep running when the glasses go and are handed back when they
return. A headset is paired once, by a code the host shows and the headset's
wearer types -- proved, never sent -- and is known by its certificate afterwards. Every host offers its own settings first, as an
ordinary window that works with the pointer or with the D-pad, A and B.

An application there can also be the room: a Second Life viewer draws the world
around you and turns it with your head, while local windows float in front of it.
If its host goes quiet, the room is taken away within three seconds rather than
left frozen round you. See [docs/remote.md](docs/remote.md).

An application's sound is carried as wide as its catalogue entry says — stereo
unless it asks for 5.1, 7.1 or 7.1.4, which the session then places at the
window like a local one's. A session also says whether glasses are on, so a
virtual-reality application can be asked to be an ordinary window when they are
not.

**The applications.** SpatiShelf, SpatiQuill, SpatiReel, SpatiLoupe, SpatiPal and
SpatiKey are built and were driven on the files and media they were written for,
with FFmpeg and each system's own hardware decoder (VA-API, MediaCodec,
VideoToolbox). SpatiMesh on Chromium runs on the Deck, on the Mac (inside the app,
video decoded by the hardware: [docs/spatimesh-mac-port.md](docs/spatimesh-mac-port.md))
and on the Beam Pro (carried in the APK, a window of the room). Their settings are kept the same on every paired machine, and each
reaches remote storage through the one storage service. SpatiPal and dictation
use models served by hemera, a model router on the owner's server. What was seen
and what was only tested is in [docs/spatiand-apps.md](docs/spatiand-apps.md),
[docs/spatimesh.md](docs/spatimesh.md), [docs/spatikey.md](docs/spatikey.md) and
[docs/ai-integrations.md](docs/ai-integrations.md); the assistant has not been
tried with a real voice in the glasses.

**On the XREAL Beam Pro** (Android 14, no root; in [android/](android/); see
[docs/beam-pro.md](docs/beam-pro.md)). The Deck's compositor runs as an ordinary
app on the glasses' display at 3840×1080, measured at 59-60 fps, with the phone
as its controller. A host's windows arrive with the Deck's title bars and come
back on reconnect. The phone's own Android apps open as windows in the room,
through a shell helper run over Spatiand's own adb client.

**On a Mac** (in [mac/](mac/); see [docs/mac.md](docs/mac.md)).
A menu-bar app that needs no glasses: a paired host's applications open as
ordinary Mac windows, with the mouse, keyboard, clipboard (text and images) and
sound going with them. Plug the glasses in and the Mac runs the room itself --
the same compositor as the Deck, built for macOS and drawn through ANGLE, so the
launcher, settings, pointer and window frames are the Deck's own. The Mac's
applications are in its launcher and their windows come into the room, clicked
and typed into as on the Mac, each with its sound placed where it is and a menu
button on its bar that opens the application's own menus as a list; its mouse,
trackpad and keyboard steer the room until Ctrl-Option-G gives them back. A
whole screen can be a window too -- one the Mac has, or a virtual one made for
the purpose, which macOS takes for a monitor. The Mac can also be a host in its
own right: a Deck or a Beam Pro pairs with it, finds its open windows and its
screens in the list of windows, and uses them like a Linux host's.

**Recording.** The HUD's *Record a video* saves what the glasses show, both
eyes side by side, remote pictures included, as Matroska in `~/Videos/Spatiand`
— H.264, 36 fps, marked left/right stereo. It carries three sound tracks, so an
edit can remix or replace any of them: what you heard after the HRTF (stereo,
the default), a 7.1.4 surround mix made before the HRTF from where each window
sits — a window on your left plays from the left speakers, a raised one from
the heights — and the microphone. The Beam Pro records the same file. On the
Deck, `kill -USR2 $(pgrep -x spatiand)` starts and stops it too.

**The panel** is a touch sidecar: volume, brightness, audio device, a second
keyboard, and the way out. A tap on the Deck's power button switches the panel
off completely — picture and backlight, not just dimmed — while its touch
sensor stays on; touching it wakes it again (that first touch only wakes, it
does not press anything). Holding the power button for a second and a half
ends the session.

## What does not work yet

Listed because finding out by hitting them is worse.

  * **Two-handed window gestures.** The geometry is written and tested; nothing
    consumes it, so moving and scaling with both thumbs does nothing.
  * **OpenXR is partial.** Spatiand has an OpenXR runtime of its own
    (`crates/spatiand-openxr`): Vulkan and OpenGL applications run, in the
    room on the Deck and, through `spatiand-host`, from another computer. It is
    not conformant, tracked hands are a gamepad standing in, and a Windows game
    needs a one-off step under Proton. [docs/openxr.md](docs/openxr.md) says
    exactly what is and is not there.
  * **Games stutter while you point at them.** A game plays with the virtual
    gamepad, but resting a thumb on a trackpad over it makes Stumble Guys
    stutter until a few seconds after the thumb lifts. The session log now
    measures the game's own frame gaps to find out why.
  * **Remote: no webcam, no stream to OBS, and no latency figures yet.** Glass
    to glass has not been measured against Sunshine and Moonlight. Copying a
    file between machines copies its path, which does not exist on the other
    one.
  * **Object audio.** Atmos and DTS:X never reach the OS as objects on Linux, so
    what arrives is channels. Real object audio needs a protocol an application
    would have to speak; it does not exist yet.
  * **The Mac and the Beam Pro are newer, and less worn-in than the Deck.** The
    glasses run at 60 Hz on the Mac rather than 72 (macOS does not offer the
    two-eye mode at 72); [docs/mac.md](docs/mac.md) and
    [docs/beam-pro.md](docs/beam-pro.md) list what has only been checked
    without glasses. The Beam Pro has no `CAPTURE_SECURE_VIDEO_OUTPUT`, so
    protected apps draw black in the room.
  * **Not run on hardware.** Of the last two days' work: SpatiKey's holders on a
    real Touch ID or BiometricPrompt (no device with a sensor but the Mac exists
    here), a locked Mac used from nowhere, the pose channel in the glasses,
    MediaCodec and VideoToolbox with raw streams. The Beam Pro's hardware-buffer
    frames were run and measured on the phone on a stand-in display
    ([docs/beam-pro.md](docs/beam-pro.md)), not in the glasses.
  * **Not packaged for distribution.** The Mac app is ad hoc signed and not
    notarised, so it is for the Mac it was built on.
  * **A secret window is not protected from the assistant.** SpatiPal is kept from
    SpatiKey's and the profile locks' windows by their ids and titles, and can
    still click and type into a secret window; a protocol flag is wanted. A locked
    Mac still takes clipboard sync.
  * **SpatiKey's holders are not signed**: a removed device that is still linked
    for sync can bury entries ([docs/spatikey.md](docs/spatikey.md)).
  * **Deinterlacing of hardware frames is a shader blend** (MediaCodec on the
    Beam Pro, VideoToolbox on the Mac), a workaround; VA-API and software use
    FFmpeg's filters. Raw AV1 (`.obu`) cannot be seeked.
  * **SpatiMesh does not show a film's own subtitle tracks.** The Mac's start-up race with
    ANGLE is worked round, not cured. What waits for the next Chromium build is
    listed in `third_party/chromium/patches/README.md`.
  * **Other glasses.** The device table lists the XREAL Air, Air 2, Air 2 Pro and
    Air 2 Ultra; only the first-generation Air has been seen working. The traits
    and a null backend exist so that other headsets are a table entry rather
    than a rewrite. Nobody has tried one.

## Getting it

Download the newest release from the
[Releases page](https://github.com/peprasetya/spatiand/releases/latest):

  * **Steam Deck**: the `.tar.gz`. Unpack it in Desktop Mode and double-click
    **Install Spatiand**.
  * **XREAL Beam Pro**: the `.apk`. Open it on the phone and allow the install.
  * **Mac** (Apple Silicon): the `.zip`. Move Spatiand.app to Applications and
    open it the first time with right-click → Open (it is not notarised).

See [docs/install.md](docs/install.md) for what each does and what to check
when something is wrong. When it is still wrong,
[open an issue](https://github.com/peprasetya/spatiand/issues/new/choose): its form asks for
what is needed to find it. More about each part is on [spatiand.com](https://spatiand.com/).

## Tested on

The Deck, below, is the machine worn in longest. Everything in its table is what
it was, not what is required. The versions are as of SteamOS build 20260716.1
(July 2026) and have not been read off the machine since.

| | |
|---|---|
| Hardware | Steam Deck LCD, AMD Custom APU 0405 (Van Gogh) |
| Glasses | XREAL Air, first generation, USB `3318:0424` |
| OS | SteamOS 3.8.16, build 20260716.1 |
| Kernel | 6.16.12-valve24.5 |
| Graphics | Mesa 25.3.0 radeonsi |
| Audio | PipeWire 1.6.4, libmysofa 1.3.3 |
| X11 | Xwayland 24.1.10 |

**XREAL Beam Pro**: model X4000, build `X4000_X502_260518_ROW`, Android 14,
Snapdragon, with XREAL Air glasses (`3318:0424`), checked 2026-09-26. Later work
was run on the phone with a stand-in display ([docs/beam-pro.md](docs/beam-pro.md)).

**Mac**: built and run on an Apple silicon Mac mini (littleotter, macOS 27.2.1,
[docs/spatimesh-mac-port.md](docs/spatimesh-mac-port.md)); it is the owner's
daily machine in the glasses since November 2026. The room was first checked in the
glasses on an Intel MacBook Pro with XREAL Air ([docs/mac.md](docs/mac.md)).

The glasses need a USB-C cable that carries video. Many do not, and one that
charges perfectly well will leave the display black.

## Building it

**Deck and other Linux.** Built in an Arch container, because SteamOS has no
toolchain and an atomic `/usr` is no place to put one.

    tools/setup-buildbox.sh          # once: an Arch distrobox with the deps
    cargo build --release            # inside it
    tools/make-release.sh            # a folder somebody can click

**Beam Pro** (SDK build-tools and cargo-ndk, no Gradle; [docs/beam-pro.md](docs/beam-pro.md)):

    android/build.sh                 # android/spatiand.apk
    android/build.sh install         # ...and onto the phone adb talks to

**Mac** (on a Mac; ad hoc signed, [docs/mac.md](docs/mac.md)):

    mac/make-app.sh                  # Spatiand.app

**SpatiMesh's Chromium** (a Linux machine with about 60 GB free per tree):

    [CHROMIUM_TARGET=android|mac] third_party/chromium/build.sh <work> [fetch|patch|build|package|all]

[docs/install.md](docs/install.md) is for installing a built release.

There is a headless renderer for looking at layout without hardware:

    SPATIAND_BACKEND=snapshot SPATIAND_SNAPSHOT=/tmp/hud.png SPATIAND_VIEW=hud spatiand

And a running session will take a picture of itself, or start and stop a
recording:

    kill -USR1 $(pgrep -x spatiand)
    kill -USR2 $(pgrep -x spatiand)

The snapshot backend can also *click*, which is the only way to open a real
application's menu without wearing anything:

    SPATIAND_BACKEND=snapshot SPATIAND_CLIENT=dolphin \
      SPATIAND_CLICK=0.09,0.03 SPATIAND_SNAPSHOT=/tmp/menu.png spatiand

`tools/popup-probe.c` is a ninety-line Wayland client that opens a window and a
menu and nothing else. It exists because menus were broken for a reason no
amount of reading found, and because the first version of it — which painted at
the first configure, like no real toolkit does — worked perfectly and proved
nothing.

## How it is put together

Thirty-six crates. The three that touch the Deck's hardware — `spatiand-hmd`,
`spatiand-input`, `spatiand-platform` — are the only ones whose *logic* knows what
a Steam Deck or a pair of XREAL glasses is. Everything else sees traits, so a
second headset should be a table entry rather than a rewrite.

    grep -ril 'steamos\|steamdeck\|xreal\|gamescope' crates/

hits more files than that, and all but one of them are comments: the reasoning
behind a constant frequently refers to the hardware it was measured on, and
hiding that would make the code less honest rather than more portable. The
exception is the prompt that asks you to plug the glasses in, which names them
on purpose — it is shown precisely when there is no headset to ask.

The room:

| crate | what it is |
|---|---|
| `spatiand` | the compositor itself |
| `spatiand-hmd` | headset: IMU, display modes, buttons, the table of devices |
| `spatiand-track` | head tracking: filter, bias, magnetic anchor, prediction |
| `spatiand-input` | controller, touchscreen, gestures, scroll |
| `spatiand-pad` | the one virtual gamepad a game sees, here or on a host |
| `spatiand-mapper` | controller layouts: bindings, templates, the editor's model |
| `spatiand-render` | GLES 3.2: stereo cameras, skybox, glass, text |
| `spatiand-shell` | scene graph, launcher, HUD, keyboard |
| `spatiand-audio` | spatial audio: geometry, HRTF, PipeWire |
| `spatiand-platform` | session install, launching, desktop settings |
| `spatiand-proto` | private Wayland protocols, `spatiand_xr_v1` among them |
| `spatiand-openxr` | an OpenXR runtime: the loader finds it, its frames become a stereo window |
| `spatiand-voice` | dictation and the assistant's ears and voice, through hemera |

Other systems (each compiles the Deck's modules and adds only the platform):

| crate | what it is |
|---|---|
| `spatiand-android` | the Deck's compositor on the glasses, with the phone as its controller |
| `spatiand-adb` | Spatiand's own adb client: pairing with a phone's Wireless debugging, running a command as its shell user |
| `spatiand-mac` | the Deck's compositor on the glasses, drawn through ANGLE |
| `spatiand-mac-core` | the link to a Spatiand host, behind a small C interface, for the Mac app |
| `spatiand-surfaces` | pictures on the GPU (IOSurfaces) handed from a process to its parent, on macOS |

Remote applications:

| crate | what it is |
|---|---|
| `spatiand-stream` | the wire to a remote host: messages, pairing, video packets, the clipboard |
| `spatiand-video` | decoding a host's pictures, and encoding recordings |
| `spatiand-host` | the remote application host, a small compositor of its own |
| `spatiand-host-catalog` | what a host keeps on disk: its applications and settings |
| `spatiand-host-config` | the host's settings, as an application the headset opens |

The SDK and the applications:

| crate | what it is |
|---|---|
| `spatiand-app` | what an application is written against: a window in the room, its input, GL, the room's own widgets |
| `spatiand-media` | what a film is and how it is played: FFmpeg, each platform's decoder, the layouts of two eyes |
| `spatiand-storage` | how an application reaches the storage service: rclone's remotes as virtual places |
| `spatiand-sync` | the applications' settings, kept the same on every paired machine |
| `spatiand-choose` | the file chooser: a window of the room that a program asks for files, a folder or a name |
| `spatishelf`, `spatireel`, `spatiloupe`, `spatiquill`, `spatipal` | the five applications: files, video, viewer, terminal, assistant |
| `spatikey`, `spatikey-core` | SpatiKey: the application and the browsers' way to it; its vault, crypto, request protocol and service, shared by the Mac app, the room and the apps |
| `vibrefy-api` | talks to one Vibrefy deployment (a media server SpatiReel plays from) |

## Documentation

| | |
|---|---|
| [docs/apps.md](docs/apps.md) | Writing an application for Spatiand: window sizing, audio layouts, and the stereo and immersive-video protocols as they are specified so far |
| [docs/spatiand-apps.md](docs/spatiand-apps.md) | Spatiand's own applications — SpatiShelf, SpatiReel, SpatiLoupe, SpatiQuill, SpatiPal, SpatiKey and the SDK they share: the plan, what was built, and what was checked |
| [docs/look-and-feel.md](docs/look-and-feel.md) | The look-and-feel rules for the first-party applications and the room, and where each departs from them |
| [docs/x11.md](docs/x11.md) | Why X11 runs and is not supported |
| [docs/input-mapper.md](docs/input-mapper.md) | Controller layouts and the virtual gamepad a game sees |
| [docs/openxr.md](docs/openxr.md) | The OpenXR runtime: how it works, how to run a game with it (native, Proton, remote), what is missing |
| [docs/remote.md](docs/remote.md) | Applications on another computer: what was measured, and what each measurement decided |
| [docs/install.md](docs/install.md) | Installing a release, and what to check when it goes wrong |
| [docs/xreal-air.md](docs/xreal-air.md) | The glasses' protocol, verified against hardware. Probably the most reusable thing here |
| [docs/remove-a-device.md](docs/remove-a-device.md) | Removing a device from the shared list of paired devices |
| [docs/steam-deck-controller.md](docs/steam-deck-controller.md) | The controller's HID reports |
| [docs/state.md](docs/state.md) | Where the project stands: what works, what has not run on hardware, what is open |
| [docs/spatimesh.md](docs/spatimesh.md) | SpatiMesh, the browser: Chromium with Spatiand's patches on all three systems, and the monthly update to the newest Chromium |
| [docs/spatimesh-mac-port.md](docs/spatimesh-mac-port.md) | Chromium's Linux user interface carried to macOS |
| [docs/spatikey.md](docs/spatikey.md) | SpatiKey: passwords and passkeys |
| [docs/ai-integrations.md](docs/ai-integrations.md) | Dictation and the assistant |
| [docs/mac.md](docs/mac.md) | The room on a Mac, and the Mac as a host |
| [docs/beam-pro.md](docs/beam-pro.md) | The room on the XREAL Beam Pro |
| [docs/privacy.md](docs/privacy.md) | What leaves the machine, and what never does |

## Licence

MIT, for what is Spatiand's own.

What it carries of others keeps their licences, listed in
[third_party/NOTICES.md](third_party/NOTICES.md). In particular
`third_party/chromium/patches/base` is the set of patches taken over from the
Cromite browser, GPL-3.0 (`COPYING` beside them), and
`third_party/chromium/android-libc/termux` is termux-packages' set for Chromium,
under Chromium's own licence; SpatiMesh as built from them is distributed under
the GPL-3.0.
