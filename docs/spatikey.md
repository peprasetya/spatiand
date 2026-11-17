# SpatiKey, Spatiand's passwords and passkeys

Asked for on 2026-10-29: a password and passkey manager of Spatiand's own. The vault is
encrypted and kept the same on every machine, as the applications' settings are, and is opened
only by a fingerprint. **Every device with biometrics can hold it** (the owner, 2026-11-17: "the
password needs a device with biometrics, so ALL devices with biometrics can have the
passwords"): each Mac with Touch ID, and an Android device with a strong fingerprint or face, once
a device that holds it has added it (or the recovery code has). A device without biometrics -- the
Deck, and the Beam Pro, which has no sensor -- has every use approved by a touch on whichever
holder answers on the same network: never over Tailscale. Today the one holder is the owner's
Mac. An iPhone companion comes later, once there is an Apple developer account.

This is the design every part is built to. What was built, and what was checked, is at the end.

## Why not Apple Passwords

Passkeys in iCloud Keychain are given only to browsers holding Apple's managed
`com.apple.developer.web-browser.public-key-credential` entitlement (a paid developer team and
Apple's approval). A macOS AutoFill credential provider, which could take passwords and passkeys
over from Apple Passwords by Credential Exchange, needs paid signing as well, as does saving
anything into iCloud Keychain from a program: synchronizable keychain items need an
application-identifier that only a provisioning profile gives. Spatiand.app is signed with the
self-signed "HoloFrame Dev" certificate. So SpatiKey is its own store, and a site's passkey made
in SpatiKey is a second passkey for that site, beside the one in Apple Passwords. With a
developer account later: the recovery code saved into iCloud Keychain by SpatiKey itself, the
credential provider, and the iPhone companion.

## Decided (2026-10-29)

| | |
|---|---|
| Name | **SpatiKey** |
| Opening the vault | **A finger only** -- Touch ID on a Mac, a strong (class 3) biometric on Android. No password or PIN standing in: with the lid shut nothing opens. |
| Where it is kept | Encrypted, in the applications' synced folder, so it reaches every Spatiand machine and host. Reading it needs the vault key, which only a holder's finger (or the recovery code) gives. |
| Who holds it (2026-11-17) | **Every device with biometrics**, each with the vault key wrapped to a key of its own behind its sensor. Added by a holder, with a touch there, or by the recovery code. |
| Deck, Beam Pro | Ask a holder: any that answers on the same network. The holder does the work and sends back only the answer. Same network only, proved per request. |
| Recovery | A recovery code shown once at setup. SpatiKey cannot save it into iCloud Keychain without a paid account, so the owner pastes it into Apple Passwords himself, which puts it in iCloud Keychain behind Face ID or Touch ID. |
| First build | The manager; SpatiMesh sign-in (passkeys, filling and saving passwords); typing an entry into any window; 2FA codes (TOTP); Chrome and Firefox extensions, installed the developer way and not published. |
| Not now | Importing from Apple Passwords' CSV (not asked for yet). |

## The pieces

```text
crates/spatikey-core   the library: vault, crypto, entries, TOTP, WebAuthn, the public suffix
                       list, the network rules, the request protocol, the service
crates/spatikey        SpatiKey, the application (an SDK window), and spatikey-host, the
                       browsers' native-messaging host
mac/Sources/Spatiand/SpatiKeyGuard.swift
                       the Secure Enclave key and Touch ID, in Spatiand.app itself
crates/spatiand-mac-core/src/key.rs
                       the service on the Mac, started with the app, behind sp_key_*
android/src/id/prasetya/spatiand/KeyGuard.java, KeyWrap.java
                       the Keystore key and BiometricPrompt, in the APK (a phone with a sensor)
crates/spatiand-android/src/android/key.rs
                       KeyGuard asked over JNI
crates/spatiand/src/spatikey.rs
                       the service on the Deck and on Android (it forwards, or holds the vault
                       where there is a sensor), and the room's half on every platform: typing,
                       the window to type into, the card
third_party/chromium   SpatiMesh's half: patch 0010 (Chromium's password store and WebAuthn on the service);
                       the Rust window program's (crates/spatimesh) was removed on 2026-11-10, and
extensions/spatikey    the Chrome and Firefox extension, built into dist/ by its build script
```

**The service** is the only thing that reads the vault. Every caller -- SpatiKey, SpatiMesh,
the browser extensions, the room -- talks to it over one local socket. On the Mac it runs inside
Spatiand.app from launch, whether or not the room is up, because the Touch ID prompt must come
from the app itself (it names Spatiand), and because SDK apps end with the compositor. On the
Deck and the Beam Pro it runs inside the compositor and forwards every request to a Mac.

## The vault

Kept in the shared folder, `~/.config/spatiand/apps/spatikey/` (`spatiand_app::config::shared()`),
which `spatiand-sync` copies between every two linked Spatiands, newest file winning.

```text
apps/spatikey/vault.json               who the vault is: {"v":1,"id":<16 hex>,"created":ms,"by":<fp8>}
apps/spatikey/entries/<32 hex>.json    one entry each: {"v":1,"id":..,"n":<b64 nonce>,"c":<b64>}
apps/spatikey/devices/<fp64>.json      each holder, a device that opens it with its own biometrics:
                                       name, kind (mac-touch-id, android-biometric), its key's
                                       public half, the vault key wrapped to it, when added, by
                                       which holder; a removed one is a tombstone ("removed": true)
apps/spatikey/requests/<fp64>.json     a device with biometrics asking to be added: name, kind, its
                                       new key's public half; buried ("closed") once answered
apps/spatikey/recovery.json            the vault key wrapped by the recovery code
local/spatikey/touch-id.json           this holder's key (a blob only its own Secure Enclave, or
                                       the Keystore alias only this app, can use) and its wrap of
                                       the vault key
local/spatikey/asking.json             while this device waits to be added: its key's blob
local/spatikey/settings.json           how long a touch lasts; how long an open SpatiKey stays open
local/spatikey/allowed.json            (a holder) devices allowed to ask it, by fingerprint and name
local/spatikey/approvers.json          (Deck, Beam Pro) the holders this device asks, likewise
```

**One file per entry**, because the synchroniser keeps the newest *file*: two entries saved on
two machines while apart must both survive, and a whole-vault file would lose one. Files are
small (an entry is padded to a multiple of 256 bytes and kept under 64 KiB) because sync drops
files over 4 MiB and a host closes a session whose message passes 1 MiB. A deleted entry is
rewritten as a tombstone (`deleted: true`, everything else emptied), not removed, so the
deletion travels as the newest version rather than depending on the synchroniser noticing.
Writes go through a hidden `.<name>.<pid>.new` file, `fsync`, then rename, and the file's time
only moves forward (the previous time plus a millisecond if the clock says otherwise).

**Keys.**

* The *vault key* (VK) is 32 random bytes and is never written anywhere in the clear.
* The *entry key* is HKDF-SHA256(VK, salt = the vault id, info = `spatikey entries v1`).
  Each entry is ChaCha20-Poly1305 under it, with a random 12-byte nonce and the associated data
  `spatikey/1/<vault id>/<entry id>`. A file moved to another name or vault does not open.
  Authenticated encryption is also what stops a host holding the synced files from forging or
  altering an entry: everything in `apps/` lands on deepmagpie too.
* **Touch ID** wraps VK to a P-256 key made inside this Mac's Secure Enclave, with access control
  `.privateKeyUsage` + `.biometryCurrentSet` (Touch ID only; adding or removing a finger retires
  the key, and the recovery code sets Touch ID up again). The key is not kept in the keychain --
  that would need entitlements a self-signed app cannot have -- but as CryptoKit's
  `dataRepresentation`, a blob only this Secure Enclave can use, in `local/`. The wrap is ECIES,
  all in Swift (`SpatiKeyGuard.swift`): an ephemeral P-256 key, ECDH with the Secure Enclave key,
  HKDF-SHA256 (salt `spatikey`, info = ephemeral public ‖ Secure Enclave public, both X9.63),
  AES-256-GCM with the associated data `spatikey touch-id v1`; stored as ephemeral public (65) ‖
  nonce (12) ‖ ciphertext (32) ‖ tag (16), base64. Unwrapping is an ECDH inside the Secure
  Enclave, which asks for the finger.
* **Every holder's wrap is that one**, whatever its kind: an Android holder's key is a Keystore
  P-256 key for key agreement, and the same ECIES is written three times -- `SpatiKeyWrap` in
  Swift, `KeyWrap.java`, and `crates/spatikey-core/src/holders.rs` (wrapping only, which needs only
  the public half: what a holder does when it adds another). `tests/known_answers.rs` opens a wrap
  CryptoKit made; a wrap Rust made was opened by CryptoKit and by `KeyWrap` (2026-11-17).
* **The recovery code** is 25 Crockford base32 characters (125 bits), shown as five groups of
  five. Argon2id (64 MiB, 3 passes, 1 lane, a random 16-byte salt) of the code, without dashes and
  in capitals, gives a key that wraps VK with ChaCha20-Poly1305 (associated data
  `spatikey recovery v1/<vault id>`). Reading it accepts small letters, spaces, and O/I/L for
  0/1/1, as Crockford's alphabet intends.

**An entry**, once decrypted (the padding is a `pad` field beside it):

```json
{"kind": "login", "title": "GitHub", "sites": ["github.com"], "apps": [],
 "username": "paulus", "password": "...", "notes": "",
 "totp": {"secret": "JBSWY3DPEHPK3PXP", "digits": 6, "period": 30, "algorithm": "SHA1",
          "issuer": "GitHub", "label": "paulus"},
 "passkey": null,
 "created": 1793241047000, "changed": 1793241047000, "changed_by": "0b8dfc23",
 "version": 1, "deleted": false}
```

`kind` is `login`, `passkey` or `note`. A passkey is its own entry:
`{"rp_id", "rp_name", "credential_id" (base64url), "user_handle" (base64url), "user_name",
"user_display", "key" (PKCS#8, base64), "alg": -7, "sign_count": 0}`. `sites` are what a page
must belong to (registrable domains, or full origins for a site that needs an exact match);
`apps` are window application ids (Steam's is `steam`), for typing into windows.

## Asking for something

The protocol is the same on the local socket and between devices: each message is a 4-byte
big-endian length and then JSON. A request is `{"id": n, "op": ..., ...}` and its answer
`{"id": n, "ok": {...}}` or `{"id": n, "err": {"code": ..., "message": ...}}`. The local socket
is `$SPATIKEY_SOCKET`, else `$XDG_RUNTIME_DIR/spatikey.sock`, else on the Mac
`~/Library/Application Support/Spatiand/spatikey.sock`. It accepts this user only (the peer's
uid is checked) and names the asking program in the prompt.

| op | what | asks for a touch |
|---|---|---|
| `status` | is there a vault; can this machine open it; which Macs could approve, and are they on this network | no |
| `setup` | make the vault on this Mac, set up Touch ID, give the recovery code | yes (to enrol) |
| `open` | open SpatiKey: a session, and every entry's summary | yes |
| `close`, `list`, `get`, `put`, `delete` | in an open session | no |
| `totp` | a 2FA code, in a session or alone | alone: yes |
| `fill` | the logins for a page's origin | yes |
| `save` | save a login a page has just used | yes |
| `passkey_create`, `passkey_get` | WebAuthn, given the origin and the hash of clientDataJSON | yes |
| `profile_unlock` | a finger's yes to opening a browser profile's locked windows; nothing of the vault leaves | yes, always a new one |
| `type` | type an entry's fields into a window | alone: yes |
| `target` | the window that had the focus before SpatiKey's own | no |
| `recovery_new`, `recover` | a new recovery code; set Touch ID up again from the code | session / the code |
| `devices`, `forget_device`, `pair` | devices that may ask this Mac, the holders and the requests; pairing | session (removing a holder: a new touch) / see below |
| `holder_request` | (a device with biometrics that does not hold the vault) make its key, ask to be added; the six digits | no |
| `holder_approve`, `holder_refuse` | (a holder) add a device that asked, if it shows the digits given; or turn it down | session and a new touch / session |
| `settings` | how long a touch lasts (0, 60 or 300 s); how long SpatiKey stays open (default 300 s) | session |

**A touch, and how long it lasts.** By default every use asks. The setting lets one touch cover
what follows within 60 or 300 seconds, by keeping the evaluated `LAContext` alive that long. An
open SpatiKey window is a *session*: one touch, and the vault stays open for that window only
until it closes or has sat unused for the idle time; the Mac keeps the entry key in memory for
it, and nothing else. A fill, a passkey, a save, a type from outside a session asks each time.

**What the prompt says.** macOS writes "Spatiand is trying to ..." and SpatiKey finishes the
sentence from the request: "sign in to github.com with a passkey", "fill in your password for
github.com on Steam Deck", "type your Steam password into Steam". The origin shown is the one the
page's engine reported (SpatiMesh) or the browser reported (the extensions), never the page's own
claim.

## Every device with biometrics holds the vault

Built 2026-11-17, before a second such device exists ("You can build now, although I have nothing
to test"): the Beam Pro has no fingerprint or face sensor and the Deck none, so the owner's Mac is
the one holder there is.

**One vault, the key wrapped once per holder.** The vault, its entries and its recovery code are
as they were. What makes a device a holder is its record in `devices/<fp>.json`: the vault key
wrapped (the ECIES of "Keys") to a key of that device's own that only its sensor lets be used --

| kind | key | used after |
|---|---|---|
| `mac-touch-id` | a Secure Enclave P-256 key, `.privateKeyUsage` + `.biometryCurrentSet` | Touch ID, per use (or the touch-lasts setting) |
| `android-biometric` | an Android Keystore P-256 key for key agreement (StrongBox where it agrees keys, else the TEE), `setInvalidatedByBiometricEnrollment` | a class-3 biometric: per use on Android 15+ (the ECDH is BiometricPrompt's CryptoObject); before Android 15, for 5 s after it (see the workarounds) |

-- and its own half in `local/spatikey/touch-id.json` (the key's blob, which nothing but that
device can use). No device credential (password, PIN) stands in for the finger on either.
Other platforms hold nothing.

**Adding a holder** needs a touch on one that holds it already, or the recovery code:

1. On the new device (a Mac with Touch ID, or an Android device with a strong biometric) SpatiKey
   says the vault has reached it and offers *Ask to be added*: its key is made, asking nobody
   (`holder_request`), its blob kept in `local/spatikey/asking.json`, and `requests/<fp>.json`
   written to the synced folder with its name, kind and public key. It shows six digits.
2. On a holder, in SpatiKey's settings, the request is listed with the same six digits. *Add it*
   asks for a new touch there -- whatever an open SpatiKey or a lingering touch would cover --
   and wraps the vault key to the request's public key (`holder_approve`), writing the new
   record; the request is buried.
3. The new device finds its record (its public key in it) when it next looks, and keeps its half:
   it opens SpatiKey with its own finger from then on. SpatiKey's window there moves on by itself.

The six digits are SHA-256 of the vault id, the device's fingerprint and its new public key: they
are what keeps a request honest, since anyone who can write the synced folder (a host holds a copy)
could leave one there with a key of their own. A holder refuses `holder_approve` before any touch
when the digits it is given are not the request's now. Or, on the new device, the recovery code
(`recover`) makes it a holder directly, as it always set a Mac up again.

**Removing a holder** is done from any other holder, with a touch there (`forget_device`): its
record becomes a tombstone (not a deleted file, so that the removal travels as the newest version
through the synchroniser). The removed device sees the tombstone and lets its half go; it can be
added again, or given the recovery code. A device cannot remove itself.

**A device without biometrics asks any holder that answers.** It pairs with each holder it may
ask, as it paired with the Mac (a holder pairs devices the same way, whatever its kind). For each
request: the holder it asked last, if it answers the broadcast within 0.3 s of the first that
does; else the first to answer. A holder that cannot ask anybody -- one that does not hold this
vault, or whose sensor cannot be used now (a Mac with its lid shut) -- answers `not_here` or
`touch_id_unavailable`, and the next is asked in its place: nobody was asked anything there. The
card in the glasses names the holder being asked: "Touch ID on darkPhoenix to save a password for
github.com", "Fingerprint on Pixel to ...".

**A phone with biometrics that does not hold the vault yet** asks a holder meanwhile, as the Beam
Pro does (`Mode::Both`): what it holds it answers itself, and until then it is a forwarder. A
request that came from another device is answered there or refused, never passed on.

**Nothing changed for the owner's setup.** The existing Mac is holder #1: its record has been in
`devices/` since setup, the vault and the recovery code are untouched, and it opens as before. The
one change in place: a holder whose record is missing from the synced folder (lost, or a vault from
before records) writes it again from its own half the first time it looks
(`a_vault_set_up_with_one_mac_is_taken_as_it_is_and_takes_a_second_holder`).

**Security, unchanged:** every unlock still needs a fresh finger on a holder (or the touch-lasts
window the owner chose); the vault key leaves a holder only wrapped to another holder's key, which
needs that holder's finger to open; on Android it is never put in a Java string (Java makes the
key, Rust wraps to it; an unlock hands the key back as a byte array that is then overwritten);
devices without biometrics never see it.

**What removing does not do**: the vault key stays the same, so a removed device that kept a copy
of its own half and its wrap (it lets them go, but a stolen one would not) could still open what it
held. The proper cure for a lost holder is a new vault key, every entry sealed again and wrapped to
the remaining holders and a new recovery code: not built.

## The Deck and the Beam Pro ask a holder

The rule the owner gave: the holder's sensor must be within reach, so a request is answered only
from the same network, and Tailscale is not that. Below, "the Mac" is any holder: what is said of
it is done by an Android holder the same way.

**An address is not proof.** Tailscale's subnet routes carry ordinary private ranges here: this
Mac reaches 192.168.10.0/24 and 192.168.11.0/24 through `utun7`, and the home 192.168.1.0/24 is
advertised into the tailnet as well. deepmagpie once saw the Beam Pro arrive as 192.168.10.28 --
almaserver, the subnet router -- an on-link neighbour by every address test. So:

* **Finding the Mac is a broadcast.** The device sends `SPATIKEY1 ASK <nonce> <fp> <name>` to
  UDP 47611 at 255.255.255.255 and at each network's own broadcast address. A broadcast does not
  cross a router or a tunnel. The Mac answers `SPATIKEY1 HERE <nonce> <fp> <port> <name>` from the
  network it arrived on, and writes down the sender, its network and the time -- but only if the
  packet was sent *to a broadcast address* (read with `IP_RECVDSTADDR`/`IP_PKTINFO`), arrived on
  an interface that can broadcast and is not point-to-point, loopback or a tunnel address
  (100.64.0.0/10, fd7a:115c:a1e0::/48), and came from inside that interface's own subnet.
  Interfaces are judged by their flags and addresses, never their names (utun numbers change).
* **Asking is QUIC** on UDP 47612, with the same pinned certificates as the rest of Spatiand
  (`spatiand_stream::transport`), the ALPN `spatikey/1`, connection migration off, one stream per
  request. For every request the Mac checks again that the peer's address is on one of its LAN
  interfaces and was heard broadcasting within the last two minutes; the device broadcasts again
  before each connection and every minute while connected. A request that fails is refused with
  `not_same_lan`.
* **A device paired with this Mac as a host may ask too**, with no second pairing (the owner, 2026-10-31): the same
  certificate, and each use still wants a finger on the Mac and the device on its network. Forgetting such a device is
  the host's to do, for now. Devices that are not hosts' pair as follows.
* **Who else may ask** is SpatiKey's own list: pairing the Beam Pro with the Mac *as a
  host* would make it take the Mac's windows over from the Deck. A device pairs with SpatiKey on a
  Mac once, on the same network: it sends `SPATIKEY1 PAIR <nonce> <fp> <name>`, the Mac lets that
  one certificate through the handshake for a minute, the device connects and asks `pair`, and
  both show the same six digits (`transport::pairing_code`, from both fingerprints). The Mac asks
  its owner with the code ("Steam Deck wants to use SpatiKey on this Mac"); the device asks its
  wearer to compare. Only when both say yes is each written down: `allowed.json` on the Mac,
  `approvers.json` on the device.
* **The Mac decides, not the device.** The Mac never trusts what a device says about being near;
  the device's own checks only save a wasted prompt.

The Beam Pro's Tailscale accepts subnet routes by default, so at home its packets to the Mac may
go into the tunnel and arrive from the router's address, and be refused. Its socket is bound to the
Wi-Fi network when Android allows it (`android_setsocknetwork`); otherwise SpatiKey says so and
names the two cures: exclude Spatiand from Tailscale, or turn off its subnet routes.

## SpatiMesh

Our WPE (SpatiMesh's engine until 2026-11-10) had no WebAuthn of its own (`ENABLE_WEB_AUTHN` was off; there was no `navigator.credentials`
or `PublicKeyCredential`), so the page's half is ours:

* **`webauthn.js`, in the page's world**, defines `navigator.credentials` (`create`, `get`,
  `store`, `preventSilentAccess`), `PublicKeyCredential` with its static functions,
  `AuthenticatorAttestationResponse`, `AuthenticatorAssertionResponse` and `Credential`. Where a
  browser already has them (the extensions) it wraps them instead, and falls back to the
  browser's own when SpatiKey has nothing for the site. It turns buffers into base64url and back,
  honours `signal` and `timeout`, and speaks only through events to the isolated world. It never
  says what the origin is and never builds clientDataJSON.
* **`cred-world.js`, in an isolated world of its own** (`spatimesh-cred`, every frame, document
  start), is the only script that can reach the message handler `spatimeshCred`: a page cannot
  post to it, cannot see its globals, and cannot change what `self.origin` returns there. It
  stamps every request with that origin, whether it is the top frame, and the top frame's origin
  when it can be read, and relays the answer. It also finds login forms (password inputs,
  `autocomplete` username/current-password/new-password/webauthn), tells the window when one has
  the focus, fills it when asked (in that frame only), and notices a login being submitted.
* **The window** builds clientDataJSON from the stamped origin, refuses anything not https or
  localhost, refuses a cross-origin frame (there is no permissions policy without WebKit's
  WebAuthn), checks the relying party id against the origin with the public suffix list, and asks
  the service. A key button beside the address bar fills the focused login; after a login is
  submitted with a password SpatiKey does not have, it offers to save it. `mediation:
  "conditional"` (passkeys offered in the login field) waits until the key button is pressed.
* **Fixed on the way:** the page description sent to dictation and the assistant no longer
  includes the value of a password field, and Copy no longer copies out of one.

The engine-verified origin a WebKit patch could give (the script message's frame, which WebKit's
GLib API drops) is the stronger alternative to the isolated world's stamp; it is not built.

## SpatiMesh on Chromium

Chromium has its own password manager and its own WebAuthn, so here SpatiKey is what they keep things in
(patch 0010); nothing is injected into pages, and the origin is always the browser's own.

**Nothing of the vault is kept on this disk**, not even which sites it has a login for, because the vault
cannot say so without a touch. `SpatikeyPasswordStoreBackend` wraps Chromium's password store:

* A login form is told there may be one: a credential with the name `SpatiKey` and a placeholder for a
  password, shown in the field as "Fill from the vault, with a touch". Chromium's fill-on-account-select is on,
  so opening a page asks for nothing.
* Choosing it asks `fill` for the frame's origin. One login is filled; several are offered in the same popup
  with no second touch, and let go of when it closes. Refused or no match says why, in an infobar on the tab.
* Submitting what was just filled saves nothing: the site and a salted hash of the password are remembered in
  memory for half an hour, so there is no false "Update password?".
* A typed login gets Chromium's own "Save password?"; Save calls `save`. `Login Data` stays empty of passwords.
  "Never for this site" and HTTP authentication stay in Chromium's own store: its dialog cannot ask for a touch.
* A page never receives the placeholder through the Credential Management API.
* "Manage passwords" and `chrome://password-manager` open SpatiKey's own window.

**Passkeys** are `device/fido/spatikey/`, the platform authenticator on Linux, as Touch ID is on a Mac.
`isUserVerifyingPlatformAuthenticatorAvailable` says yes when there is a vault this machine opens or a Mac to
ask. No dialog comes first: the service's own touch names the site, and several accounts go to Chromium's
account sheet. A request with no list of its own is told of one passkey standing for all of them, which is also
the line offered in a login field for `mediation: "conditional"`. Security keys are reached by Chromium's dialog
when the site asks for cross-platform or after a failure.

The program runs as `spatimesh`, not `chrome`, because the service names who is asking from what is running.

Checked in the Deck's test room against `examples/soft-service` (2026-10-30): a passkey made, signed in with and
without the site's list, refused when excluded; a login filled, chosen from two, submitted with no save question;
a typed login saved through the bubble with `Login Data` at 0 rows; Manage passwords opening SpatiKey. Not checked:
a real touch, and the Deck asking the Mac over the air.

## The extensions

`extensions/spatikey` builds `dist/spatikey-chrome` (Manifest V3) and `dist/spatikey-firefox`
(Manifest V3 with Firefox's background scripts, id `spatikey@prasetya.id`). The same two scripts
run in them: `webauthn.js` as a `world: "MAIN"` content script, wrapping the browser's own, and
the forms half as an ordinary content script. The background script asks `spatikey-host` (native
messaging), which relays to the service's socket. **The origin comes from the browser**
(`sender.origin`), never from the page. Installed the developer way: Chrome's "Load unpacked"
(the manifest carries a fixed key, so the extension's id, and the host manifest naming it, stay
the same), and Firefox Developer Edition with signatures off, or a temporary add-on in Firefox.
`tools/spatikey-browsers.sh` writes the native-messaging host manifests for Chrome, Chromium,
Brave, Edge and Firefox on the Mac and on Linux.

## Typing into a window

SpatiKey's entry page has "Type into <window>": the window that had the focus before SpatiKey's
own. The room types it with the seat keyboard, as dictation does, but strictly: a character the
room's keyboard cannot type stops the whole thing before a key is sent (dictation would quietly
drop it), and nothing is typed unless the focus is still on that window. Mac windows are typed as
Unicode key events by the Swift half, so any character and any keyboard layout work; there the
room names the window itself (not only its application), the Swift half brings it forward and
asks Accessibility whether the frontmost application's focused window is that one, before the
first piece and before each one after, and types nothing more when it is not.

## The holder's half

`SpatiKeyGuard.swift` holds the Secure Enclave key and asks for Touch ID; the service in
`spatiand-mac-core` calls it through one callback and takes the answer back (`sp_key_start`,
`sp_key_answer`), the pattern of `SP_DESCRIBE`. On Android `KeyGuard.java` is asked the same
questions in the same JSON (`android/key.rs`, over JNI). Both go through spatikey-core's
`platform` (`AppGuard`): unlock, enroll (the Mac's), can-touch, allow a device, and make a key
with nothing wrapped to it (to ask to be added). When the glasses are on the Mac, macOS's own
Touch ID panel is meant to be brought into the room as a "macOS asks" window; the owner found it
staying on the Mac's virtual screen, out of sight (2026-11-16), which is why the card below is
there whoever asks.

## The card while a finger is wanted

Asked for on 2026-11-16: "I do not see the dialog that needs the Touch ID. It shows in the virtual
screen, out of my awareness... such a dialog should be shown in the glasses... even if I use
Deck/Beam Pro... But I do not want it to conflict with other features."

**Whenever SpatiKey waits for a finger on the Mac, the room the wearer is in shows a card saying
so**: "SpatiKey / Touch ID on your Mac to save a password for github.com". The same on every
platform, from one piece of code:

* **What it says, and when** is spatikey-core's (`Service::with_card`): the Mac's service puts it
  up through its guard around every Touch ID prompt -- unlocking and enrolling, for this Mac's own
  programs and for other devices' ("... to open SpatiKey in Chrome on Steam Deck") -- and the Deck's
  and the Beam Pro's forwarding service around every request to the Mac that may want a finger
  (`finger_wanted`). It goes when the answer comes. If the finger did not come -- cancelled, not in
  time, Touch ID not there, the fingers changed -- the card's last word says so for four seconds
  ("Did not save a password for github.com: Touch ID was cancelled"); any other refusal is the
  asking program's to say. Two requests waiting at once on a device: the newer's card, then the
  older's again when the newer is answered.
* **How it is shown** is the room's (`crates/spatiand/src/spatikey.rs`, compiled into the Deck's,
  the Beam Pro's and the Mac's room alike): it shows once the asking has gone on 0.3 s (a touch
  still covered by the one before is answered at once, and a card for a frame says nothing), for
  two minutes at most. **It goes over every other card** -- dictation's, the assistant's, the
  Mac's or the phone's notifications, a host's -- and, since each frame asks SpatiKey first and
  the others only when it is empty, the card it covered is back as it was once it goes. A press on
  it does nothing and reaches nothing under it (`owns_notice`): dictation's text is not typed into
  a terminal, a notification is not opened. It takes no focus and no input.
* **The Mac's desktop (2D) shows no card**: the room draws it only in the glasses, and on the
  desktop macOS's own panel is in view. The Mac's room has the card once the glasses have been on
  in that run of the app (the room sets the hook on its first frame). On the Deck's desktop session
  (`desktop_service`) the card and its last word are the desktop's notifications.

**Why the Mac's room showed nothing before**: the Mac's service put a card up only for a request
*from another device* (`unlock_lingering`'s `elsewhere`). A request from the Mac itself -- SpatiMesh
saving a password, SpatiKey's window opening -- was taken to be seen in macOS's panel on the Mac's
screen, which in the glasses is not seen.

Looked at in the snapshot room with `SPATIAND_SNAPSHOT_TOUCH=<seconds>[,no]`: SpatiKey's service in
the room's process as the Mac runs it, at `SPATIKEY_SOCKET`, with a new vault and Touch ID stood in
for by a wait that then says yes (or no); a program on the socket asks, and the frame is taken while
it waits or after.

Checked on 2026-11-16, in deepmagpie's (the Deck's) and littleotter's (the Mac's) snapshot rooms with
SpatiQuill and dictation into it: the card over dictation's waiting text, a press on it typing
nothing; dictation's card back once the touch was answered, and a press then typing its text; the
last word after a "no". Unit tests for what the card says and when, two cards at once, the Mac's
own asking through the app's callback, and the room's timing. Not checked: a real Touch ID panel,
the glasses themselves, the Deck and the Beam Pro asking a real Mac (the Beam Pro's room is the same
code, type-checked).

## What was built, and what was checked (2026-10-29)

Built: everything above. `crates/spatikey-core` (vault, recovery code, service, socket, WebAuthn,
TOTP, the public suffix list, the network rules and the Deck's and Beam Pro's asking);
`crates/spatikey` (the window and `spatikey-host`); `SpatiKeyGuard.swift` and
`spatiand-mac-core/src/key.rs` (the service in Spatiand.app from launch); the room's half
(`crates/spatiand/src/spatikey.rs`, with Mac windows typed as Unicode by `SpatiKeyTyping.swift`);
SpatiMesh's half (`browser/keys.rs`, `credentials/cred-world.js`, the engine's message handler);
the Chrome and Firefox extensions (`extensions/spatikey`, `tools/spatikey-browsers.sh`).

| Checked | How |
|---|---|
| The library | 91 tests, the cross-module ones included (`cargo test -p spatikey-core -- --include-ignored --skip a_real_broadcast --skip a_kept_connection`) |
| WebAuthn's output | signatures verified with ring and OpenSSL; the attestation object read back |
| Same network only | a real broadcast on this Mac's Wi-Fi (en0) answered and paired in one process; utun7 never counted as a LAN |
| The Mac's Secure Enclave key | made with `.biometryCurrentSet` and restored, with no prompt; the wrap format against a software key |
| The service in Spatiand.app | starts at launch, its socket answers `status` (no vault yet, Touch ID can set one up) |
| SpatiMesh on WPE, passkeys | in the test room against `examples/soft-service` (Touch ID stood in for): a page made a passkey and signed in with it, clientDataJSON's origin the isolated world's |
| SpatiMesh on WPE, passwords | the key button lit on a focused login field; pressed, the right login filled |
| The extensions | 50 tests, in headless Chrome and Firefox (`node --test extensions/spatikey/test/*.test.js`) |
| The window | every screen in the test room with `--demo` |

Not yet checked: a real touch (setting the vault up is the owner's, in SpatiKey's window, as it
shows the recovery code); the Deck and the Beam Pro asking the Mac over the air; typing into
windows in the live room; the extensions against the real `spatikey-host`; the save question
after a real login.

**Every device with biometrics (2026-11-17): what was checked.** `tests/holders.rs`, with holders
whose keys are software P-256 keys doing the very ECDH a Secure Enclave or a Keystore does: a phone
asking, added by the Mac with a touch (refused before the touch for other digits, nothing added
on a "no"), then opening the same vault with its own finger and what each saves the other reads; a
holder removed with a touch, letting its half go, and made one again by the recovery code; a vault
laid out as the single-Mac SpatiKey left it, taken as it is (no file rewritten, the same recovery
code) and given a second holder; a Deck paired with both asking over QUIC on loopback, the card
naming whichever answered, the one asked last asked again, the other asked when the first's sensor
cannot be used, and the one left when the other leaves the network. The wrap format three ways
(CryptoKit on littleotter, Android's `KeyWrap` on a JVM, Rust). The soft-service rig with two
holders over one folder (`soft-service <folder> <socket> <name> android`), driven over their
sockets: asked, added, both opening, removed. `cargo ndk check` of the Beam Pro's room, and the
APK's Java compiled. **Not checked:** a real Touch ID or BiometricPrompt, any Android device with a
sensor (none here), a second Mac, the window's new screens in the room.

**Workarounds, with the proper alternative.** Before Android 15 a Keystore key agreement cannot be
BiometricPrompt's CryptoObject, so a key made there is usable for 5 s after the finger rather than
for that one use (the proper one, per-use, is what Android 15+ gets, and it is chosen when the key
is made). Android's prompt has no "touch lasts" of its own: each unlock asks. The origin of a SpatiMesh request is the isolated
world's stamp, not the engine's (a WebKit patch exposing the script message's frame); a Touch ID
panel that got no answer is taken down by Swift at 110 s, before Rust gives up at 120 s (a cancel
over the C interface); changed fingers are told from a finger that did not match by how long the
attempt took when the Secure Enclave's error says nothing more (keep the biometry state hash in
`touch-id.json` and compare it); the Beam Pro's socket is bound to its Wi-Fi address only
(`android_setsocknetwork` with the Wi-Fi network from Java).
