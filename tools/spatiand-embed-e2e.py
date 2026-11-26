#!/usr/bin/env python3
"""A window inside another's place (spatiand_xr_embed_v1), end to end, with no GPU, no display and
no glasses.

The room's compositor (SPATIAND_BACKEND=snapshot) runs the SDK's own example of both sides
(crates/spatiand-app/examples/embed.rs): a blue host with a place in its middle, which starts
itself again as the orange window inside. SPATIAND_STEPS presses inside the place and types, then
presses the host and types, and the room's picture is taken at the end:

    the place is made and the window goes inside it, offered the place's size
    a press inside gives it the keyboard; the key it is typed is its own
    it says so to the host, which answers, and the answer reaches it (it turns green)
    a press on the host gives the keyboard back to the host
    the room shows one window, with the green inside drawn at the place, in the middle

    cargo build -p spatiand -p spatiand-app --example embed
    tools/spatiand-embed-e2e.py [target/debug] [out-folder]
"""

import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("e2e", os.path.join(here, "e2e_png.py"))
e2e = importlib.util.module_from_spec(spec)
spec.loader.exec_module(e2e)

# The example's colours, as the room draws them (red, green, blue).
BLUE = (0x20, 0x50, 0xC0)
GREEN = (0x30, 0xC8, 0x40)


def main():
    target = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/debug")
    out = os.path.abspath(sys.argv[2] if len(sys.argv) > 2 else "target/spatiand-embed-e2e")
    os.makedirs(out, exist_ok=True)
    work = tempfile.mkdtemp(prefix="spe2e-")
    home = os.path.join(work, "home")
    runtime = os.path.join(work, "run")
    os.makedirs(home)
    os.makedirs(runtime, mode=0o700)
    room = os.path.join(out, "room.png")
    env = dict(os.environ)
    env.update({
        "XDG_RUNTIME_DIR": runtime,
        "HOME": home,
        "EGL_PLATFORM": "surfaceless",
        "SPATIAND_BACKEND": "snapshot",
        "SPATIAND_SNAPSHOT": room,
        "SPATIAND_CLIENT": os.path.join(target, "examples", "embed"),
        "SPATIAND_HMD": "null",
        "SPATIAND_INPUT": "off",
        "SPATIAND_STEPS": ";".join([
            "wait:4", "window:Embed host",
            # The place is the middle half of the host: (200, 125) to (600, 375) of 800 by 500.
            "click:0.5,0.5", "wait:1", "type:a", "wait:2",
            "click:0.1,0.1", "wait:1", "type:b", "wait:2",
        ]),
        "RUST_LOG": "info",
    })
    env.pop("DISPLAY", None)
    log = os.path.join(out, "spatiand.log")
    with open(log, "w") as f:
        subprocess.run([os.path.join(target, "spatiand")], env=env, stdout=f, stderr=subprocess.STDOUT, timeout=300)
    apps = os.path.join(home, ".local", "share", "spatiand-apps.log")
    said = [line.split("] ", 1)[-1].strip() for line in open(apps, errors="replace")] if os.path.exists(apps) else []
    shutil.copy(apps, os.path.join(out, "apps.log")) if os.path.exists(apps) else None
    shutil.rmtree(work, ignore_errors=True)

    failed = []

    def check(what, ok):
        print(("ok    " if ok else "FAIL  ") + what)
        if not ok:
            failed.append(what)

    def at(line):
        return said.index(line) if line in said else -1

    check("the place is made, with a token", "host: a place, its token 32 long" in said)
    check("the window goes inside it", "host: a window is inside the place" in said)
    check("offered the place's size first", "inside: ready, offered Some((400, 250))" in said)
    check("a press inside reaches the window inside", "inside: a press" in said)
    check("and gives it the keyboard", at("inside: keyboard here") > at("host: a window is inside the place"))
    check("the key typed there is its own", "inside: a key (30)" in said and "host: a key came to the host (30)" not in said)
    check("what it says reaches the host", 'host: the window inside says "key 30"' in said)
    check("and the host's answer reaches it", 'inside: the host says "green"' in said)
    check("a press on the host gives the keyboard back", at("host: keyboard here", ) >= 0 and at("inside: keyboard gone") >= 0)
    check("the key typed then is the host's", "host: a key came to the host (48)" in said and "inside: a key (48)" not in said)
    if os.path.exists(room):
        w, h, rows = e2e.read_png(room)
        green = e2e.count(rows, 0, 0, w, h, lambda p: e2e.near(p, GREEN, 30))
        blue = e2e.count(rows, 0, 0, w, h, lambda p: e2e.near(p, BLUE, 30))
        check(f"the room shows the inside, green ({green} pixels), within the host, blue ({blue})", green > 20000 and blue > green)
        # The inside's middle is the host's middle: the place is centred.
        xs = [x for y in range(0, h, 4) for x in range(0, w, 4) if e2e.near(rows[y][x], GREEN, 30)]
        bx = [x for y in range(0, h, 4) for x in range(0, w, 4) if e2e.near(rows[y][x], BLUE, 30)]
        if xs and bx:
            centre, host_centre = sum(xs) / len(xs), (min(bx) + max(bx)) / 2
            check(f"at the place: centred on the host ({centre:.0f} by {host_centre:.0f})", abs(centre - host_centre) < 30)
    else:
        check("the room's picture was taken", False)
    leftover = subprocess.run(["pgrep", "-f", os.path.join(target, "examples", "embed")], capture_output=True, text=True).stdout.split()
    check("nothing of the test is left running", not leftover)
    print(f"pictures and logs in {out}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
