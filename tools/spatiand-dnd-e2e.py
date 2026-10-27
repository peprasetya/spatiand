#!/usr/bin/env python3
"""Dragging between windows in the room, end to end, with no GPU, no display and no glasses.

The room's compositor (SPATIAND_BACKEND=snapshot) runs two SpatiShelf windows and a SpatiMesh
one (WebKitGTK), and SPATIAND_STEPS drags between them, the button held from one window to the
next as a wearer's would be:

    shelf-to-shelf   a file from one SpatiShelf onto a folder in another: moved there (the same
                     disk), the folder under the drop lit while it is over it
    shelf-to-mesh    a page from SpatiShelf onto SpatiMesh's tabs: opened in a new tab
    held             a drag still held when the room is photographed: its card in the room

    tools/spatiand-dnd-e2e.py [target/debug] [out-folder]
"""

import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("e2e", os.path.join(here, "spatimesh-e2e.py"))
e2e = importlib.util.module_from_spec(spec)
spec.loader.exec_module(e2e)

SHELF = (1440, 900)
PAGE = '<!doctype html><title>Dropped page</title><body style="margin:0;background:#8f6bf2">dropped</body>'


def shelf_at(x, y):
    return f"{x / SHELF[0]:.4f},{y / SHELF[1]:.4f}"


def main():
    target = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/debug")
    out = os.path.abspath(sys.argv[2] if len(sys.argv) > 2 else "target/spatiand-dnd-e2e")
    os.makedirs(out, exist_ok=True)
    work = tempfile.mkdtemp(prefix="spatiand-dnd-e2e-")
    home = os.path.join(work, "home")
    a, b = os.path.join(home, "A"), os.path.join(home, "B")
    os.makedirs(a)
    os.makedirs(os.path.join(b, "Inbox"))
    open(os.path.join(a, "letter.txt"), "w").write("hello")
    open(os.path.join(a, "page.html"), "w").write(PAGE)
    runtime = os.path.join(work, "run")
    os.makedirs(runtime, mode=0o700)
    # The first tile of each folder, at the top left of the pane; the files sort by name.
    first, second = shelf_at(450, 250), shelf_at(648, 250)

    def run(name, script, steps, snapshot):
        launcher = os.path.join(work, f"{name}.sh")
        open(launcher, "w").write("#!/bin/sh\n" + script + "wait\n")
        os.chmod(launcher, 0o755)
        env = dict(os.environ)
        env.update({
            "XDG_RUNTIME_DIR": runtime,
            "HOME": home,
            "EGL_PLATFORM": "surfaceless",
            "SPATIAND_BACKEND": "snapshot",
            "SPATIAND_SNAPSHOT": snapshot,
            "SPATIAND_CLIENT": launcher,
            "SPATIAND_STEPS": ";".join(steps),
            "RUST_LOG": "info",
        })
        env.pop("DISPLAY", None)
        log = os.path.join(out, f"spatiand-{name}.log")
        with open(log, "w") as f:
            subprocess.run([os.path.join(target, "spatiand")], env=env, stdout=f, stderr=subprocess.STDOUT, timeout=300)
        return open(log).read()

    shelf = os.path.join(target, "spatishelf")
    # The second program has its own connection to the room: the descriptor handed down is
    # the first's.
    two_shelves = f"{shelf} {a} &\nsleep 2\nenv -u SPATIAND_APP_FD {shelf} {b} &\n"
    lit = os.path.join(out, "shelf-over.png")
    log1 = run("shelf-to-shelf", two_shelves, [
        "wait:6", "window:A",
        f"down:{first}", f"move:{shelf_at(470, 260)}", f"move:{shelf_at(520, 280)}", "wait:1",
        "window:B", f"move:{shelf_at(900, 600)}", f"move:{shelf_at(455, 255)}", "wait:1", f"dump:{lit}",
        f"up:{shelf_at(455, 255)}", "wait:3",
    ], os.path.join(out, "room-shelf-to-shelf.png"))

    mesh = f"env -u SPATIAND_APP_FD SPATIMESH_HOME={os.path.join(work, 'mesh')} SPATIMESH_ENGINE=webkitgtk {os.path.join(target, 'spatimesh')} &\n"
    after = os.path.join(out, "mesh-after.png")
    log2 = run("shelf-to-mesh", f"{shelf} {a} &\nsleep 2\n{mesh}", [
        "wait:8", "window:SpatiShelf",
        # page.html is first now that letter.txt has gone.
        f"down:{first}", f"move:{shelf_at(470, 260)}", f"move:{shelf_at(520, 280)}", "wait:1",
        "window:SpatiMesh", "move:0.5,0.5", "move:0.3,0.03", "wait:1",
        "up:0.3,0.03", "wait:5", f"dump:{after}",
    ], os.path.join(out, "room-shelf-to-mesh.png"))

    held = os.path.join(out, "room-held.png")
    log3 = run("held", two_shelves, [
        "wait:6", "window:A",
        f"down:{first}", f"move:{shelf_at(470, 260)}", f"move:{shelf_at(520, 280)}", "wait:1",
        "window:B", f"move:{shelf_at(900, 600)}", "wait:1",
    ], held)

    failures = []

    def check(what, ok):
        print(("ok    " if ok else "FAIL  ") + what)
        if not ok:
            failures.append(what)

    check("shelf-to-shelf: the room had a drag", "a drag began, with a picture" in log1)
    check("shelf-to-shelf: it was taken", "a drag ended: taken" in log1)
    check("shelf-to-shelf: the file is in the folder it was let go of over", os.path.exists(os.path.join(b, "Inbox", "letter.txt")))
    check("shelf-to-shelf: and not where it was (moved, on one disk)", not os.path.exists(os.path.join(a, "letter.txt")))
    if os.path.exists(lit):
        _, _, rows = e2e.read_png(lit)
        # The folder under the pointer, lit blue over its own green.
        blue = e2e.count(rows, 365, 170, 535, 335, lambda p: p[2] > p[1] + 25 and p[2] > 140)
        check(f"shelf-to-shelf: the folder under it lit ({blue} blue pixels)", blue > 4000)
    else:
        check("shelf-to-shelf: photographed while over the folder", False)
    check("shelf-to-mesh: it was taken", "a drag ended: taken" in log2)
    if os.path.exists(after):
        _, _, rows = e2e.read_png(after)
        violet = e2e.count(rows, 100, 300, 1500, 900, lambda p: e2e.near(p, (0x8F, 0x6B, 0xF2), 20))
        check(f"shelf-to-mesh: the page dropped on the tabs is open ({violet} pixels of it)", violet > 200000)
    else:
        check("shelf-to-mesh: photographed after", False)
    if os.path.exists(held):
        w, h, rows = e2e.read_png(held)
        # The card hangs below and right of straight ahead, the middle of the picture: what is
        # dragged is page.html (letter.txt went into the Inbox), a page of code, whose colour
        # is a green nothing else there is.
        card = e2e.count(rows, w // 2 - 40, h // 2 - 40, w // 2 + 220, h // 2 + 220, lambda p: p[1] > p[0] + 30 and p[1] > p[2] + 10)
        check(f"held: the card in the room ({card} pixels of it)", card > 1500)
    else:
        check("held: the room photographed", False)

    shutil.rmtree(work, ignore_errors=True)
    print(f"pictures in {out}")
    if failures:
        print(f"{len(failures)} check(s) failed")
        sys.exit(1)


if __name__ == "__main__":
    main()
