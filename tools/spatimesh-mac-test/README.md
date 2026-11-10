# SpatiMesh on the Mac: the test rig (littleotter)

Copied to `/tmp/smtest` on littleotter (`scp -r` this folder there; the media are made with
ffmpeg on deepmagpie, see below). SpatiMesh is the build in
`~/Developer/spatiand-chromium-mac/chromium/src/out/mac-ozone`, run as a Wayland client of
Spatiand's **snapshot room** (`crates/spatiand-mac`'s `spatiand-mac`, `SPATIAND_BACKEND=snapshot`),
which needs no glasses, no screen and no unlocked Mac, and writes a PNG.

- `go.sh <png> <wait s> [SpatiMesh flags and URLs]`: the snapshot room with SpatiMesh in it.
  `SOUND=1` gives the room its sound (`SPATIAND_SNAPSHOT_SOUND=1`): the room's log
  (`/tmp/smtest/room.log`) then says which window each stream is placed at
  (`sounds through slot`, `heard as spatiand.window.N`, `placed through slot N`).
  `SPATIAND_STEPS="wait:N"` keeps it running. SpatiMesh's own log is `/tmp/smtest/sm.log`
  (pages' `console.log` lines appear in it).
- `live.sh <url>`: into the running Spatiand.app instead (needs its compositor up: glasses, or
  one of its applications opened from the menu).
- `crash.py [n]`: the newest SpatiMesh crash report's stack, symbolised by macOS.
- `media/matrix.html`, `media/reuse.html?f=a,b,...`: each file played for 3 s, with what was
  seen (time, size, frames, error) logged as `[codec] {...}`. `play.html?f=`: one file, looped.
- `ext-two-windows`: two windows with a near-silent tone each, then the second's tab moved to the
  first window (`--load-extension`, fresh profile). `ext-three-tabs`: three media tabs in two windows.
- The media: `testsrc2` + a quiet `sine`, 20 s, in each container/codec pair (`ffmpeg ... -f
  mpegts`, `-c:v libx265 -tag:v hvc1`, `libvpx-vp9`/`libopus`, `libsvtav1`, `ac3`, `mp2`).

Results of 2026-11-11 are in docs/spatimesh-mac-port.md.
