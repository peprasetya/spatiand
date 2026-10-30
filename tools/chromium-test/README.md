# Testing SpatiMesh's Chromium on the Deck

What the checks in docs/spatimesh.md and docs/spatikey.md were made with. Never the real data: the shared
folder is a copy in `/tmp/crt-config`, the browser's own in `/tmp/crt-keep`, and with `skroom.sh` the vault is
`crates/spatikey-core/examples/soft-service`, which says yes to every touch.

Copy `crsync.sh` to `~`, the others to `/tmp`, and `pages/` to `/tmp/crt-skpages`; `~/crt-wrap.sh` starts
`~/spatiand-chromium/run.sh` with its log in `/tmp/crt-chrome.log`.

    room.sh <fresh|keep> <seconds> [chrome args and URL]     the test room; fresh copies the folder again
    skroom.sh <fresh|keep> <seconds> "[chrome args and URL]" the same with the fake SpatiKey and pages on :18765
    crq.py 9333 <URL|-|tab:part of URL> <JS or @Cdp.method> [picture.png]    asks the browser over DevTools
    skask.py /tmp/crt-sk/sock <op> [json]                    asks the fake service
    set-pin.py /tmp/crt-config <profile id> <pin>            gives a profile of the copy a known PIN

`STEPS` is the room's `SPATIAND_STEPS`: `wait:9;click:0.2,0.37;type:abc;key:28`.

Things that cost time:

* Start the room through `room.sh`, not `nohup` or `setsid`: the session's end kills everything else, and a browser
  killed that way loses the last half minute of cookies.
* Name the profile (`--profile-directory=Default`) after opening another: the last one used opens, and with a lock
  every click goes to its PIN window.
* Bubbles are in the room's picture (`/tmp/crt/room.png`, written when the steps end), not in `dump:`, which is the
  toplevel's own texture.
* `coredumpctl info <pid>` gives a symbolled stack even on this stripped build; an official build's failed CHECK is
  a bare SIGTRAP with nothing in the log.
