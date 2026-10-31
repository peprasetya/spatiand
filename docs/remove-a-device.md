# Removing a device

Your devices share one list. A device paired with any of your machines is paired with all of
them, and a device removed on any machine is removed on all of them. If a device is lost or stolen,
remove it from whichever machine you can reach first.

Removing takes effect on a machine as soon as that machine hears of it: at once if it is connected to
the machine you removed it on, otherwise the next time it connects to one that knows. The removed
device is hung up on, loses its windows and sync, and can no longer ask SpatiKey.

**To see the list on any platform**, use `--devices`, which prints the start of each fingerprint:

```bash
spatiand-host --devices
```

## A Linux host (such as deepmagpie)

From a shell on the host, by the start of the fingerprint or by the device's name:

```bash
spatiand-host --forget e74e570c
```

## A Mac

In the menu bar: **Spatiand > Let other devices use this Mac's windows > Your devices > Remove...**

Or from a terminal:

```bash
~/Applications/Spatiand.app/Contents/MacOS/Spatiand --forget-device e74e570c
```

## A Steam Deck

In the room: open **Remote computers**, go to the host's row, and press forget on it twice. This removes a
*host*. To remove any other device, use a shell (Desktop Mode, or ssh):

```bash
/home/deck/spatiand/target/release/spatiand --forget-device e74e570c
```

## A Beam Pro

In the room: open **Remote computers**, go to the host's row, and forget it the same way. It has no
terminal, so remove any other device from one of your other machines.

## If the device was stolen

1. Remove it, from any machine you can reach, as soon as you can.
2. Change what it could see. Removal stops it from getting anything new, but whatever it had already
   synced stays on its disk unencrypted: storage logins, media server tokens, assistant chats,
   bookmarks, history and cookies. SpatiKey's vault stays encrypted, and opening it takes your
   finger on your Mac.
3. Check the list with `--devices` on another machine.

**There can be a race.** Whoever holds a paired device can also remove your other devices from the list
until it has been removed itself. Your other machines recover with physical or shell access: pair a
device again with `spatiand-host --pair` on a host, or **Let a new device pair** on a Mac. Pairing again
is the owner's word, and puts it back on the list.

## Getting back settings that another machine deleted

Each machine keeps what another machine deleted in a private bin for 30 days, up to 20 MB. The bin is
never synced.

```bash
spatiand-host --settings-bin
```

```bash
spatiand-host --restore-settings 24
```

That restores everything deleted in the last 24 hours and syncs it back to your other machines. Give
a path too, to restore less, for example `--restore-settings 24 spatimesh/`. On a Mac or a Deck, use
the program named above in place of `spatiand-host`.
