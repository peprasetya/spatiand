# Controller layouts

Spatiand's answer to Steam Input: every physical control is remappable, per application, and
what a game sees is one gamepad. Written because Game Mode already works this way and the
muscle memory should carry over — and because a spatial session has two inputs Game Mode has
never had, the glasses' gyro and their temple buttons.

Code: `spatiand-mapper` (all of the model and the engine, no hardware), `spatiand-input`
(`virtual_pad`, `gamepad`), `spatiand::controls` (what joins them to the session).

---

## 1. What a game sees

**One gamepad.** A uinput device wearing Steam's own virtual gamepad identity
(`28de:11ff`, "Steam Virtual Gamepad"). It is created when the session starts, so a game
launched later finds it, and it goes when the session ends.

The identity is the whole trick, and it was wrong for a while. A wired Xbox 360 pad
(`045e:028e`) is the obvious choice — every game knows it — and it is invisible: Steam hands
every game it launches a list of several hundred controller ids that Steam Input handles, with
`045e:028e` among them, and Proton honours that list. **[verified]** with Stumble Guys running,
by creating one pad of each identity at the same moment and watching which node `winedevice.exe`
opened: the Steam one within four seconds, the Xbox one never. The wire format either way is an
Xbox pad's, so this is what the device is *called*, not what it is.

**And nothing else.** Every application Spatiand launches gets
`SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT=0x28de/0x11ff` and
`SDL_GAMECONTROLLER_ALLOW_STEAM_VIRTUAL_GAMEPAD=1`. The second is not optional: SDL hides a
Steam virtual gamepad unless it is told not to, on the understanding that Steam will offer the
same controller through its own API — true for a game Steam launched, false for anything
Spatiand launched itself. **[verified]** with one pad up and one variable changed: one joystick
with it, none without. Steam passes its own environment to
every game it starts, so a game launched through Steam inherits it too. **[verified]** on the
Deck under Proton 11: with it set, the only XInput device Wine created was the virtual pad;
without it, Wine also read the Deck's controller straight from hidraw and the game saw two.

**The Deck's own gamepad disappears by itself.** While Spatiand holds the vendor hidraw
interface, `hid-steam` unregisters the `Steam Deck` and `Steam Deck Motion Sensors` devices.
**[verified]**

**And a game really does see it.** **[verified]** on the Deck by asking SDL, which is what a
game asks: with the pad held up by `cargo run -p spatiand-input --example virtual-pad`, an SDL
program listed `Steam Deck 28de/1205` and `Xbox 360 Controller 045e/028e`; with the environment
above, it listed only the second, as a game controller, and received every A press and the full
sweep of the left stick. The kernel half is a test of its own —
`the_kernel_publishes_it_as_a_gamepad`, ignored unless run on the Deck — which also checks that
an ordinary process can *open* the device node, since the permission to do so comes from a udev
rule and not from anything here. It arrives about 40 ms after the device does.

**Four axes are reserved.** The pad publishes twelve: the six a gamepad has, two for the
D-pad's hat, and four spares (`ABS_RUDDER`, `ABS_WHEEL` and the two tilts) that rest at centre
and that nothing writes yet. They exist from the beginning because the shape of a uinput device
is fixed when it is created and a game enumerates it once — an axis added later would be
invisible to everything already running. What they are for is a head: three of them can carry
yaw, pitch and roll to an application that can bind an axis but knows nothing about a headset,
which is the shortest path to driving something like Second Life from where the wearer is
looking. **[verified]** that adding them changes nothing a game sees today: SDL still reports
`Xbox 360 Controller 045e/028e` as a game controller, and `jstest` lists the twelve axes.

**A game that does not believe it is focused ignores all of this.** Wine decides whether its
window is in the foreground from `WM_TAKE_FOCUS` and `_NET_WM_STATE_FOCUSED`, and a game that
thinks it is in the background reads no gamepad at all. Both are set by giving the seat the X11
window rather than its Wayland surface — see [x11.md](x11.md), "Keys went nowhere".

---

## 2. Steam has to be told to keep its hands off

Steam takes the controller for Steam Input and zeroes every field Spatiand reads — see
`steam-deck-controller.md` §3. `steam -nojoy` ("Disable controller support", from
steamclient.so's own switch list) is the way out: **[verified]** Steam then holds no hidraw
node, creates no virtual pad of its own, logs no controller activity, and still launches games.

So `crate::controls::without_steam_input` puts `-nojoy` into any launch command whose program is
`steam` — the desktop entries Steam writes for games are `steam steam://rungameid/<appid>`.

---

## 3. The model

Steam Input's, with its names:

| | |
|---|---|
| **Action set** | a whole layout; an action can switch between them |
| **Layer** | overrides some controls while held or toggled on |
| **Binding** | what one button does: a list of activators |
| **Activator** | when it fires — regular, long press, double press, start press, release press, chorded — with optional toggle and turbo |
| **Action** | what it does: a gamepad button, stick direction or trigger, a key, a keyboard shortcut, a mouse button or wheel, an action set or layer, or a Spatiand command |
| **Mode** | what an analogue source does: joystick, directional pad, mouse, scroll wheel, flick stick, radial menu, trigger, or gyro |
| **Mode shift** | that source runs in another mode while a button is held |

Sources are both sticks, both triggers, the Deck's gyro and the glasses' gyro. Buttons are the
Deck's, plus the glasses' two temple buttons, plus a trackpad's presses by fingers and depth where
a controller reports them (section 8).

**The trackpads are not sources, and never will be.** They are Spatiand's pointer in every
application, and their clicks are its mouse buttons -- unless the application's layout binds a
press of a trackpad that counts its fingers (section 8), and then only away from the room's own
things. This was learned the short way: for one
build a layout could take them, a Steam game's layout did, and opening OpenTTD left the wearer
with no way to point at anything — including the menu that would have undone it. A game that
wants a pointer gets the one the session already has, which is what almost every game with
mouse support actually wants. Resting a thumb on a pad is still bindable, as the natural switch
for gyro aiming, and the face buttons still click *unless* the layout has claimed them.

**A plain press waits when it has to.** If a button also has a long press or a double press, its
regular press fires as a short tap on release. Firing it immediately — the other reading — makes
every long press also a short one.

**STEAM and `⋯` are not bindable.** They open Spatiand's menus in every layout, as they open
Steam's in Game Mode, so no layout can shut the wearer inside a game.

### Templates

*Gamepad*, *Gamepad with gyro aiming*, *Keyboard and mouse*, *Spatial desktop*. A Steam game
starts on *Gamepad*; anything else starts on *Spatial desktop*, which is how Spatiand's desktop
has always behaved — the triggers click the pointer, the buttons type — expressed as a layout.

**Where layouts are kept.** One file per application (`app-<app id>.toml`, `steam-<id>.toml`).
Since 2026-11-14 they are read first from `~/.config/spatiand/apps/layouts/`, the folder every
paired machine keeps the same (`spatiand-sync`), and then from `~/.config/spatiand/layouts/`,
where they were all kept before; the editor saves into the first. Nothing is moved: a layout made
before goes on working where it is, and reaches the other machines the first time it is saved
again (or copy it into `apps/layouts/` by hand). "Use the default layout" removes it from both.

**Layouts are per application, not per controller.** One layout holds the bindings of every
controller there is -- the Deck or a gamepad, the glasses, a Mac's trackpad -- so one application
can be played with a gamepad and have its trackpad clicks bound too. The editor groups the inputs
by device and offers only those of a device that is here: *Buttons* (the controller's and the
glasses'), *Trackpad clicks* (only where a trackpad reports them).

---

## 4. The editor

Settings → **Controller layout**, for whatever is in front of the wearer. Laid out as Steam's
configurator is: the controller drawn with every binding labelled around it, and a list —
buttons, then each analogue source with its mode and that mode's settings, then action sets and
layers. D-pad to move, left and right to change a value, A to open, B to go back. Changes apply
at once, so a sensitivity can be felt while it is being set, and are saved when the editor
closes.

The picture is **above** the card rather than beside it: a headset is short of vertical field,
but a card wide enough to read leaves no room beside it at all. (It is the Deck's: a trackpad's
presses are not drawn on it.)

**A keyboard shortcut is one action.** *Keyboard shortcut* in the action picker lists the common
ones (Ctrl+C, Ctrl+Shift+T, Alt+Left, ...); once chosen, the activator's page has *Held with it*
(every combination of Ctrl, Shift, Alt and Super) and *Key* to make any other. In the file it is
what a person writes: `{ type = "shortcut", keys = "ctrl+shift+t" }` (any case; a key by the
picker's name, or by its evdev number). It is pressed as the modifiers and then the key, and let
go of as the key and then the modifiers -- the room orders every frame's keys that way, so
several *Keyboard key* actions on one activator make a shortcut too.

---

## 5. Where the sources come from

**The Deck** — as always, raw hidraw (`steam-deck-controller.md`). Its trackpads go to the
pointer rather than to the layout, and a trigger the layout uses reads as unpulled to the
pointer, so one control never does two things.

**A Bluetooth or USB pad** — evdev, found by rescanning `/proc/bus/input/devices` every two
seconds, so a pad paired mid-session joins by itself. Each is grabbed (`EVIOCGRAB`) while open,
so its presses reach only the layout. Its buttons arrive as the Deck button in the same place,
which is what makes one layout cover either device. Microsoft pads are read by label because
xpad reports X and Y under the codes the kernel's own layout document assigns to the top and
left buttons the other way round.

**The glasses** — the tracker's own gyro samples, bias removed, in the head frame. Their temple
buttons only ever report a press, never a release, so a press is held for 120 ms.

**Rumble** goes the other way: a game uploads a force-feedback effect to the virtual pad and the
kernel hands it to us on the same descriptor. `poll_rumble` answers those requests — uinput
blocks the game until it is answered — and the strengths are sent to the Deck's body motors with
`ID_TRIGGER_RUMBLE_CMD`. That report's layout is from `hid-steam.c` and has **not** been
confirmed by feel. **[inferred]**

---

## 6. Gyro conventions

`Rates` is in the holder's terms: pitch positive aiming up, yaw positive turning left, roll
positive with the right side going down. Each source converts into that.

The Deck's IMU has Z out of the screen (face up reads +1 g), X right and Y towards the top edge,
so pitch is X, yaw is Z and roll is Y — SDL's Steam Deck driver maps it the same way. The signs
come from that driver and have not been checked by turning a Deck here, which is why every gyro
mode has invert switches. **[inferred]**

---

## 7. Looking at it without hardware

```text
SPATIAND_BACKEND=snapshot SPATIAND_SNAPSHOT=/tmp/editor.png SPATIAND_VIEW=controller spatiand
```

`SPATIAND_VIEW_PAGE=<row label>` opens that row first — `Buttons`, `Right trackpad`, `Gyro`.

The card shows six rows at a time and the Deck has twenty-four buttons, so most of the Buttons
page is below the fold — including the four back paddles, which are bindable like everything
else and were taken for missing because nothing said the list went on. Every page longer than
the card now says where you are in it: "7 of 24" in the footer, beside the button hints.

The pad itself can be held up without a session at all, which is how anything outside Spatiand
gets to be asked whether it can see it:

```text
cargo run --release -p spatiand-input --example virtual-pad        # 30s, pressing A once a second
cargo run --release -p spatiand-input --example virtual-pad 120 quiet
```

Then open a game's controller-binding screen, or run `jstest /dev/input/jsN`, and watch.

## The sticks are touch-sensitive, and now bindable — 2026-09-20

The Deck senses a thumb resting on either stick, the way it senses one resting on a trackpad.
The bits were noted in `spatiand_input::layout` a while ago as "almost certainly capacitive
stick-touch sensing. Nothing needs them yet" and left unassigned, so nothing in the
configurator could be bound to them.

They are `Control::LStickTouch` and `Control::RStickTouch` now, and `Button::LStickTouch` and
`Button::RStickTouch` in a layout — which means they appear in the editor's button list and on
the diagram like everything else, and `GyroEnable::WhileHeld` can hang off one. Reach for the
stick and the gyro wakes; let go and it sleeps. That is the arrangement a Steam Input user
expects, and it was the reason for asking.

Two cautions, both written into the table:

- **Which is left and which is right is a guess**, taken from the order every other pair in
  that table follows. It has not been told apart by hand. If the gyro wakes for the wrong
  thumb, those two lines swap.
- **A capacitive sensor reading a thumb that is nearly touching flickers.** Bit 47 was seen
  sitting high at rest often enough to have been filtered as noise once. Anything gating on
  these should expect a flicker rather than a clean switch.

## Hover is reported, always — 2026-09-20

A thumb resting over an application played with the gamepad used to withdraw the pointer
entirely: a game hears hover as the wearer picking up a mouse and stalls switching over, so it
was only given the pointer once something was clicked.

That guess cost more than it saved. A viewer with a real interface in it — Firestorm — needs
hover to work at all, and where the pointer is is not something to be clever about. The
pointer now behaves like a mouse on a desk, in every application. If a pad game stalls on this
again it comes back as a switch in *that application's* layout, not as a rule about all of
them; `Pointers::withdraw` and `Controls::drives_pad` are kept for exactly that.

## The gyro can aim a VR game's hand — 2026-10-05

A gyro group's output has a fourth kind besides mouse, camera and tilt: **VR right hand**
(`GyroOutput::Hand`, `kind = "hand"` in a layout file). A VR game under Spatiand's OpenXR runtime has no
hand to track, and the runtime stands the pad in for a pair of controllers (`docs/openxr.md`); this lets
the gyro turn the right one. The engine integrates the gyro's turning into `[right, up]` angles, held to
75 degrees either way and easing back to zero in about a quarter of a second once the gyro is off, and
puts them in `Frame::hand_aim`. They leave as the pad's first two spare axes (`Report::extra[0..2]`,
-1..1 for -90..90 degrees), which is why it works on the Deck, over a stream to a host, and from a Mac's
or Android's DualShock 4 without anything else. Sensitivity, deadzone, which rotation is left and right,
the inversions and what enables it are the gyro's own settings. The *VR game* template is a gamepad with
this on while a thumb rests on the right trackpad.

## 8. A trackpad's clicks, by fingers and depth — 2026-11-14

The owner's Magic Trackpad clicks, and pressed on through the click, clicks deeper (macOS's
"Force click"). He wanted each kind of click -- by how many fingers, ordinary or deep -- to do
something of his choosing in each application: a keyboard shortcut, mostly.

**The inputs.** Ten buttons: `pad_click1` .. `pad_click5` (a click with one to five fingers) and
`pad_deep_click1` .. `pad_deep_click5` (the same, pressed on to the deep click), shown in the
editor as "Trackpad: click, 2 fingers", "Trackpad: deep click, 3 fingers", under their own
*Trackpad clicks* row. That row is there only while a controller says it reports such presses
(`ControllerState::reports_pad_presses`): the Mac's, once its trackpad has said a press. The
Deck's and the phone's report none. (The Deck's pads have a real pressure, `Pad::pressure`, and
could report deep presses through the same `PressReader` one day.)

**Unbound, nothing changes.** Every template binds none of them, and a layout file that names
none loads and behaves as before. An unbound press is the click macOS made of it, as it always
was, in the same frame: one finger the pointer's left button, two the right (macOS's secondary
click, whether it is set to two fingers or to a corner), three the room's three-finger drag,
four and five whatever they did. A deep press is nothing beyond the click beneath it.

**Bound, the binding has it -- for that application, away from the room's own things.** Where
the press lands decides first (`Controls::route_pad_press`, the same in every room loop): on a
title bar or its buttons, the keyboard, a notice, a menu, or with a drag going on, the press is
the room's and the layout never hears of it, so the room can always be worked. Anywhere else it is
the application's; if the focused application's layout binds that press, the click it made is
held back from the pointer (only a button's going down; a button already held, such as the
three-finger drag, stays held), and the layout reads the press from the next frame -- one frame
late, for a bound press only. An unbound one is never late.

**Deep is on top of the click.** macOS has always sent the click before the deep click. So the
layout sees the click go down, then the deep click go down while the click stays down, then the
deep click up, then the click up. Two ways to bind them:

* *Click on Regular press, deep click on anything*: the click acts at once (it can drag, select,
  hold), and a deep click adds its own action on top. The usual choice.
* *Click on Release press*, deep click bound: the click acts only as it is let go, and **a deep
  click replaces it** -- the engine swallows the release of a click that a deep click went down
  on (`Engine`'s `swallowed`). The trade-off: a click that acts on release is a tap; it cannot
  hold or drag. Better for a binding that is a shortcut than for one that is a mouse button.

**Two fingers.** macOS never gives two fingers a deep click (measured on the owner's pad: no stage
2 with two fingers), but how hard they press is still reported: an ordinary click peaks at 0.01 to
0.12, a hard press reached 0.99 (one press measured). So a two-finger deep click can be read from
the pressure, with a tap of the pad (`.levelChange`) as the line is crossed, because nothing else
would tell the hand it happened. It is a setting of its own on the Mac (Settings -> Control ->
*Deep click with two fingers*), **off until the owner has felt it**, with *How hard* from 0.3 to
0.95, 0.6 to begin with. Once a press has gone deep it does not again until it is let go
(hysteresis: it stops being deep below half the line). The reading is `spatiand_input::PressReader`,
in the core; the Mac only says what its pad says (`sp_pad_press(fingers, stage, pressure,
two_finger_deep)`).

**The Mac's part.** `RoomTap.swift` counts the fingers on the pad as a press begins (the raw
MultitouchSupport frames, palms left out) and keeps that count until it ends, whatever lands or
lifts meanwhile; a press begins at whichever comes first of macOS's mouse-down and its first
pressure stage, and ends when both are over. A mouse's click has no fingers on a pad and is no
press. The Mac's 2D desktop mode is macOS's own and is not touched.

**Room commands** that can be bound, for these or any button: open settings, the launcher, the
keyboard, a screenshot, recentre, and since this: close the window in front, hide it, show the
open windows, dictate (the keyboard's microphone key), start or stop recording
(`controls::command_effect`).

Not yet tried with a real trackpad (the Swift side was written on Linux and is built on the Mac
at night), nor the two-finger tap's feel.
