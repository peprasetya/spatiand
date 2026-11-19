Spatiand — installing
=====================

A 3D spatial desktop for XR glasses. This file is about installing a release on a
Steam Deck, the first system it ran on and the only one with a packaged release.
The XREAL Beam Pro and a Mac are built from the source: see "On the Beam Pro and
on a Mac" at the end.

What you need
-------------

This has only ever been run on one combination of hardware. It may well work on
others; nobody has tried.

  * Steam Deck (LCD or OLED — tested on an LCD, AMD Custom APU 0405)
  * XREAL Air, first generation (USB 3318:0424). Air 2 uses the same protocol
    on the same interfaces and is likely to work; it has not been tested.
  * SteamOS 3.8.16 or thereabouts
  * A USB-C cable that carries video. Many do not.

Installing
----------

  1. Unpack the folder anywhere.
  2. Double-click "Install Spatiand".
     KDE will ask whether you trust it the first time — say yes.
  3. Type your password when asked. It is needed once, to add "Spatial Mode"
     to the list of sessions you can log into.

Nothing is installed outside your home folder except that one session entry.

Running it
----------

Plug the glasses in *before* starting, then either double-click "Go To
Spatiand" on the desktop, or log out and choose "Spatial Mode".

To leave: hold any button on the panel's exit row. From a terminal:

    steamosctl switch-to-desktop-mode plasma.desktop

If something goes wrong
-----------------------

The session writes everything it did to:

    ~/.local/share/spatiand-session.log        this session
    ~/.local/share/spatiand-session.log.1      the one before

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

**Nothing starts at all.** SteamOS updates replace `/usr` and take the session
registration with them. Run the installer again; everything else survives.

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

Neither has a packaged release: each is built from the source and installed by
the one who built it.

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
