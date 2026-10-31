# Your devices, your data

**Pair only your own devices, with your own user account, on each platform.**

Spatiand treats a pairing as "this is also me". It has no guest pairing. Pairing a device or a
computer with one of yours does all of this:

- It can show and control the windows of your computers, and they can do the same with it.
- Spatiand's shared settings are copied to it and kept in step, in both directions. That includes
  storage logins, media server tokens, assistant chats, and SpatiMesh's bookmarks, history and cookies.
- It can change or delete those settings on all your devices.
- It can ask SpatiKey for your passwords and passkeys. Each use still takes your finger on your Mac,
  with both on the same network, and it cannot open the vault itself.
- It is paired with all of your other machines automatically.

So do not pair with a friend's computer, a shared or work account, or any machine whose other users
you would not trust with your logins.

**On a computer with several users**, `spatiand-host` runs for one user account. Pairing joins your
device to that user, not to the computer. Other users on it have their own host and their own list,
and cannot reach yours.

**Where it lives.** Your settings travel only between your paired devices, directly, over encrypted
connections that check each device's certificate. No Spatiand server is involved. On each device they
are stored as ordinary files readable by your user account. Only SpatiKey's vault is encrypted on disk.

**If a device is lost,** remove it from any other machine: see [Removing a device](remove-a-device.md).
Removal stops anything new from reaching it, but what it already holds stays on it, so change any
logins it could see.
