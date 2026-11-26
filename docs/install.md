Spatiand — installing on a Steam Deck
=====================================

A 3D spatial desktop for XR glasses: your windows and Spatiand's own
applications around you in the glasses, driven from the Deck's controls. This
file comes with the Steam Deck release (tools/make-release.sh makes it). The
XREAL Beam Pro and a Mac have packages of their own; see "On the Beam Pro and on
a Mac" at the end.

What you need
-------------

This has only ever been run on one combination of hardware. It may well work on
others; nobody has tried.

  * Steam Deck (LCD or OLED — tested on an LCD, AMD Custom APU 0405), in
    desktop mode, with SteamOS 3.8 (the release is built on 3.8.28)
  * XREAL Air glasses, first generation (USB 3318:0424). Air 2 uses the same
    protocol on the same interfaces and is likely to work; it has not been
    tested.
  * A USB-C cable that carries video. Many do not: one that charges perfectly
    well can leave the glasses black.
  * A password for the Deck's user, to add "Spatial Mode" to the login screen.
    A Steam Deck has none until one is set: open Konsole and type `passwd`.

What is in the folder
---------------------

    Install Spatiand      double-click: installs it, or updates the one installed
    Uninstall Spatiand    double-click: removes it
    install.sh            the same two, from a terminal (--help says their
    uninstall.sh            options)
    spatiand/             the programs
    README.txt            this
    LICENSE               Spatiand's own licence (MIT)
    THIRD-PARTY.txt       what else is inside, and under which licence
    SOURCE.txt            where every part's source is, and how it was built

The programs are the room (`spatiand`), SpatiShelf (files), SpatiReel (video),
SpatiLoupe (pictures, PDFs and 3D objects), SpatiQuill (terminal), SpatiMesh
(the web browser: Chromium, in spatiand/lib/spatimesh, whose PDF viewer
SpatiLoupe shows PDFs with), SpatiPal (the companion), SpatiKey (passwords and
passkeys), the file chooser and the storage service, with the libraries they
need that SteamOS does not have: FFmpeg's.

Installing
----------

  1. In desktop mode, unpack the folder anywhere (in Dolphin: right-click the
     download, Extract, Extract archive here).
  2. Double-click "Install Spatiand". KDE asks the first time whether to run
     it — say yes. A terminal window says what it does.
  3. Type your password when asked. It is needed once, to add "Spatial Mode"
     to the sessions at the login screen.

The installer also offers udev rules for the glasses. Say no unless you start
Spatiand over ssh: they let it open the glasses and the controller outside a
login session, and Spatial Mode does not need them.

What is installed:

    ~/.local/share/spatiand/release          the programs
    the application menu, under Spatiand     Spatiand's applications, to run on
                                             the desktop without glasses
    "Go To Spatiand"                         on the desktop, and in the menu
                                             under System
    /usr/local/bin/spatiand-session          Spatial Mode, at the login screen
    /usr/share/wayland-sessions/spatiand.desktop
    /etc/udev/rules.d/99-spatiand.rules      only if you said yes to the rules

Nothing else outside your home folder. To update, unpack a newer release and
double-click its "Install Spatiand": your settings stay as they are.

Running it
----------

Plug the glasses into the Deck's USB-C port *before* starting, then either
double-click "Go To Spatiand" on the desktop, or log out and choose "Spatial
Mode".

To leave: hold any button on the exit row of the Deck's screen. From a
terminal:

    steamosctl switch-to-desktop-mode plasma.desktop

Removing it
-----------

Double-click "Uninstall Spatiand" in the release folder, or run

    ~/.local/share/spatiand/release/uninstall.sh

It removes the programs, their menu and desktop entries, and — with your
password — Spatial Mode's session entry, and the udev rules if you agree. What
you made with it stays, as it does when any program is removed:

    ~/.config/spatiand               settings, and SpatiKey's passwords
                                     (apps/spatikey)
    ~/.config/spatiand-chromium      SpatiMesh's profiles: bookmarks,
                                     history, logins
    ~/.local/share/spatiand          environments and applications added to
                                     the room

If something goes wrong
-----------------------

The session writes everything it did to:

    ~/.local/share/spatiand-session.log        this session
    ~/.local/share/spatiand-session.log.1      the one before
    ~/.local/share/spatiand-apps.log           Spatiand's applications

The second one is where a crash will be, because a session that dies is
restarted within seconds and the new one opens the first file fresh.

**A black screen in the glasses** with everything else working is usually one of
three things, in the order worth checking:

  * The battery is low. Below about 15% the Deck stops giving the glasses
    enough power for their panel while USB and video keep working.
  * The cable. Reseat it, then flip the plug, then try another cable.
  * The port has stopped offering video. `cat /sys/class/drm/card0-DP-1/status`
    says `disconnected` and `.../modes` is empty. Only a full shutdown clears
    this — not a restart.

**Spatial Mode is gone from the login screen.** SteamOS updates replace `/usr`
and take the session entry with them. Double-click "Go To Spatiand": it notices,
asks for your password and puts it back. Everything else survives an update.

**The installer says libraries are missing.** The release is built for SteamOS
3.8; a much newer or older SteamOS may lack a library it was built against, and
the installer says which before it changes anything.

Turning things off
------------------

`~/.config/spatiand/prefs.toml`, written on first run:

    keyboard_click            the on-screen keyboard's click
    spatial_audio             whether each window's sound is placed
    audio_directness_centred  how much unplaced sound to keep, 0 to 1
    audio_directness_off_axis the same, for windows off to one side
    pointer_hold_degrees      how far the pointer is held where a button went
                              down (0.4 if absent; 0 turns it off)
    pointer_hold_mouse_degrees  the same for a mouse's own movement (0.1)
    steadiness                how still the room is held while the head sways:
                              off, light, medium (the default) or strong; the
                              Look and feel card's Steadiness row writes it, and
                              SPATIAND_STEADY=0|light|medium|strong overrides it
    look                      system, light or dark: the Look and feel card's
                              Look row
    pip_corner, pip_size      where pinned windows sit and how big: the Look and
                              feel card's Pinned windows list

The settings in the glasses group these. *Look and feel* holds the look,
steadiness, the environment and pinned windows (the lines above, as before).
*AI* holds what more than one app uses: hemera's address, the models (the
everyday one, the larger one, the terminal clean-up one and the one that reads
text in pictures, each chosen from hemera's own list), dictation's languages
and words, and the two languages text in pictures is translated between. They
are kept in

    ~/.config/spatiand/apps/ai.json

the folder every paired machine shares, so they are the same on all of them;
the room's dictation and SpatiPal read them there, and any app through the SDK.
Where SpatiPal's older `voice.json` and `assistant.json` are in that folder,
`ai.json` is made from them the first time the room or SpatiPal reads it; they
are then left as they are, and a change made in an older SpatiPal afterwards is
not carried over.

`~/.config/spatiand/session.env` is read into the session's environment if it
exists. `RUST_LOG=debug` there is the way to get more out of the log on a
machine whose only screen is the thing being debugged.

On the Beam Pro and on a Mac
----------------------------

Each has a package of its own, and each is also built from the source by the
one who installs it:

  * XREAL Beam Pro (Android 14). `android/build.sh install` builds the APK and
    installs it on the device adb is talking to; it needs the Android SDK's
    build-tools and the NDK (no Gradle), and `SPATIAND_CHROMIUM_ANDROID` pointing
    at a packaged SpatiMesh if the browser is to be inside. No root is needed or
    used. What the phone must allow (display over other apps, notification
    access, wireless debugging for the phone's own apps) and what does not work
    there is in docs/beam-pro.md.

  * A Mac (Apple Silicon or Intel). `mac/make-app.sh` builds the release, signs
    it and puts it at ~/Applications/Spatiand.app; `SPATIAND_APP_DEST` names
    another place, and `SPATIAND_SPATIMESH_APP` the SpatiMesh to carry inside
    (without it the app is installed with no browser). It is signed for the Mac
    it was built on and is not notarised. The permissions macOS asks for and its
    notifications switch are in docs/mac.md.

`prefs.toml`, described above, is the same file on all three, in each system's
own settings folder.
