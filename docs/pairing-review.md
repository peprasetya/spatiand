# Pairing, sync and ports: review of 2026-10-31

Read from the code, commit 5c81d80; nothing was attacked or run. The owner's model: **pairing joins a user across
devices; `spatiand-host` belongs to one OS user.** The code has no concept of a user: trust is per-role, per-install
certificates. Linux keeps two identities (host, session); a Mac has one.

Sound already: mutual pinning over TLS 1.3 with whole fingerprints; a Mac host serves nothing before its owner says yes;
SpatiKey's per-request same-network proof with tunnels refused; `safe()` keeps sync inside `apps/`, so prefs, identity and
`local/` never travel; vault entries are authenticated and encrypted; two Linux users' hosts are cryptographically apart.

| # | Severity | Finding | Where |
|---|---|---|---|
| 1 | High | The Linux host streams pictures, sound and clipboard to an unconfirmed stranger while pairing is open: gated on `attached`, not `admitted`; a refusal does not close the link | `spatiand-host/src/main.rs:478,905,1000`, `clipboard.rs:173` |
| 2 | High, design | Pairing a host merges all of `apps/` both ways, rclone and media tokens included, with no membership: a flood between linked pairs. Pair with a friend's host and it can read, delete or overwrite; a planted rclone remote may run commands (inferred) | `spatiand-sync/src/lib.rs:6,217`, `session.rs:388` |
| 3 | High | A failed read or a file over 4 MiB becomes a tombstone dated now, which spreads and beats every peer, vault files included | `spatiand-sync/src/lib.rs:116-143` |
| 4 | High | No working revocation: forget does not close the live link, is local to one host, a Mac can only forget all, a device cannot drop an approver | `main.rs:569`, `host.rs:556`, `mod.rs:500` |
| 5 | High, bug | A Mac host silently drops files from a session: `bytes` arrive as base64 and `sp_settings_said` parses them strictly. Deletes pass, content does not | `host.rs:104`, `settings.rs:28` |
| 6 | Medium | The six-digit code can be ground by someone in the path: no commitment before it is shown (inferred, not performed) | `transport.rs:427` |
| 7 | Medium | A second OS user's host dies on the fixed port 47600; nothing allocates or advertises another | `spatiand-host/src/main.rs:388` |
| 8 | Medium | A Linux machine's host and session are two identities, so "pair the user" cannot hold there | `main.rs:74`, `mod.rs:375` |
| 9 | Medium | Any LAN device can raise an owner prompt with a name it chooses (SpatiKey's `PAIR`) | `remote.rs:452,621` |
| 10 | Medium | Sync allows 4 MiB, the control stream 1 MiB: one file between them stops a session's input while pictures flow | `lib.rs:27`, `net.rs:395` |
| 11 | Low-medium | A stranger's arrival closes the current session before being judged | `net.rs:159`, `host.rs:270` |
| 12 | Low | Wall-clock conflict rule: far-future times win, replay rolls back, no cap on count | `lib.rs:263,298` |
| 13 | Low-medium | The Mac's yes names no device and its alert has no timeout (partly inferred) | `host.rs:519` |
| 14 | Low | `/tmp` fallbacks without `XDG_RUNTIME_DIR` | `spatiand-host-catalog/src/lib.rs:40` |

Suggested order: 1, 3 and 5 are contained fixes. Then the user model (2, 4, 8): a user key made once and carried to a new
device at pairing, signing a roster with revocations; sync and SpatiKey only inside a roster; a host outside it is a
guest, windows only. Then commit-and-reveal for the code (6), then per-user ports (7).

**Fixed on 2026-10-31:** 1 (nothing leaves a Linux host until the owner confirms, refusing hangs up), 3 (only what is
certainly gone is a deletion, and a mass disappearance is not; a private bin of 30 days and 20 MB), 4 and 2's list (a
signed list of the owner's devices, `spatiand-sync/src/roster.rs`: paired with one machine is paired with all, removed
on one is removed on all, a removal changes the key and hangs up), 5. The owner chose **one kind of pairing, no guests**,
with the warning in `docs/privacy.md`; so 2's "friend's machine" stands as a thing the docs warn of, not something
prevented. Checked across the real Mac, deepmagpie and Deck: a device paired on one reached the other in seconds, and one removed on the Mac left deepmagpie as fast. Still open: 6, 7, 8 (a Linux machine's host and session stay two entries), 9 to 14.

**Since this review:** a device paired with a Mac as a host may ask its SpatiKey with no second pairing (one pairing now
guards more, which makes 1, 4 and 6 weigh more). Forgetting such a device is still the host's forget-all.
