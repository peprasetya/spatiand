#!/usr/bin/env python3
"""SpatiLoupe's PDFs shown by SpatiMesh's viewer inside its window, end to end, with no GPU, no display
and no glasses.

The room's compositor (SPATIAND_BACKEND=snapshot) runs SpatiLoupe on a scratch folder holding two
PDFs and a picture. For a PDF SpatiLoupe keeps a place in its window and asks the room to start
SpatiMesh into it (spatiand_xr_embed_v1, ask_system open_embedded): the Chromium program
SPATIAND_CHROMIUM_BIN names, started by a run.sh of the test's own with a profile of its own. Each
case is a run of its own, the room photographed at its end (SPATIAND_SNAPSHOT), SpatiLoupe's own
picture along the way (dump:):

    shown     the PDF in SpatiMesh's viewer, in SpatiLoupe's window under SpatiLoupe's bar; the room's
              speech told it is a PDF shown by SpatiMesh, by name
    picture   the arrow key to the next file, a picture: the place let go of, SpatiMesh's window
              closed, the picture shown
    back      on to the picture and back: the PDF again, in a new place
    next      past the picture to the other PDF: that one shown, the first one's SpatiMesh closed
    resized   the window made smaller: the place follows it
    info      the info pane open: the viewer leaves its column
    early     back to the folder before SpatiMesh's window has come (SpatiMesh slowed): the place
              let go of at once, and SpatiMesh's window, when it comes naming that place, asked
              by the room to close -- no window of its own
    apart     a SpatiMesh too old for places (patch 0035 not in it: the SDK's example standing in,
              bound to spatiand_xr_v1 12 under SpatiMesh's application id): its window of its own
              left be, the room telling the place at once (refused), SpatiLoupe back at its folder
              saying so; and a SpatiMesh that opens nothing: after 60 s the view says so, with
              Open again
    keys      a press in the viewer gives SpatiMesh the keyboard (the arrow key there is not
              SpatiLoupe's: the PDF stays); a press on SpatiLoupe's bar gives it back (the arrow
              key goes to the picture)
    tools     a press on the viewer's annotate button: its tools' side panel, Chromium's own
    closed    SpatiMesh ended under it: SpatiLoupe says so with Open again, and Open again shows it
              again
    remote    with a storage service (argument 3): a remote's PDF copied into SpatiLoupe's cache
              folder and shown from there

    tools/spatiloupe-pdf-e2e.py [target/debug] [out-folder] [spatiand-storage]

SPL_CASES=shown,keys runs only those; SPL_ANNOTATE=u,v is where the viewer's annotate button is.

Nothing of anyone else's is touched: a home, a runtime folder and SpatiMesh profiles of the test's
own, removed after; the SpatiMesh processes it started are ended by their profile's path, and
looked for after.
"""

import base64
import importlib.util
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zlib

sys.dont_write_bytecode = True
here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("e2e", os.path.join(here, "e2e_png.py"))
e2e = importlib.util.module_from_spec(spec)
spec.loader.exec_module(e2e)

CHROMIUM = os.environ.get("SPATIAND_CHROMIUM_BIN", "/media/shared/Linux/App/spatimesh-linux/chromium/src/out/deck/chrome")

# The colours each file is made of, as the room draws them (red, green, blue).
ORANGE = (242, 140, 26)   # the first PDF's band
GREEN = (51, 178, 77)     # the second PDF's
VIOLET = (143, 107, 242)  # the picture


def write_pdf(path, rgb, words, pages=3):
    """A PDF of `pages` A4 pages, each a band of `rgb` and a line of `words` in Helvetica."""
    bodies = {1: b"<< /Type /Catalog /Pages 2 0 R >>", 3: b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"}
    kids, number = [], 4
    r, g, b = (c / 255 for c in rgb)
    for page in range(pages):
        content = (f"{r:.3f} {g:.3f} {b:.3f} rg 60 300 475 420 re f 0 0 0 rg BT /F1 40 Tf 60 760 Td ({words}, page {page + 1}) Tj ET").encode()
        bodies[number + 1] = b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream"
        bodies[number] = f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R >> >> /Contents {number + 1} 0 R >>".encode()
        kids.append(number)
        number += 2
    bodies[2] = f"<< /Type /Pages /Kids [{' '.join(f'{k} 0 R' for k in kids)}] /Count {pages} >>".encode()
    out, offsets = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n"), {}
    for k in sorted(bodies):
        offsets[k] = len(out)
        out += f"{k} 0 obj\n".encode() + bodies[k] + b"\nendobj\n"
    xref, count = len(out), max(bodies) + 1
    out += f"xref\n0 {count}\n0000000000 65535 f \n".encode()
    for k in range(1, count):
        out += f"{offsets[k]:010d} 00000 n \n".encode()
    out += f"trailer\n<< /Size {count} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    open(path, "wb").write(out)


def write_png(path, rgb, w=640, h=480):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    rows = b"".join(b"\0" + bytes(rgb) * w for _ in range(h))
    open(path, "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
                           + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def main():
    target = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/debug")
    out = os.path.abspath(sys.argv[2] if len(sys.argv) > 2 else "target/spatiloupe-pdf-e2e")
    storage = os.path.abspath(sys.argv[3]) if len(sys.argv) > 3 else None
    os.makedirs(out, exist_ok=True)
    if not os.access(CHROMIUM, os.X_OK):
        print(f"no SpatiMesh Chromium at {CHROMIUM} (SPATIAND_CHROMIUM_BIN)")
        sys.exit(2)
    work = tempfile.mkdtemp(prefix="spatiloupe-pdf-e2e-")
    home = os.path.join(work, "home")
    docs = os.path.join(home, "Docs")
    os.makedirs(docs)
    first, picture, second = os.path.join(docs, "1 report #1.pdf"), os.path.join(docs, "2 harbour.png"), os.path.join(docs, "3 letter.pdf")
    write_pdf(first, ORANGE, "A report")
    write_png(picture, VIOLET)
    write_pdf(second, GREEN, "A letter")
    remote_root = os.path.join(work, "remote")
    os.makedirs(os.path.join(remote_root, "Shared"))
    write_pdf(os.path.join(remote_root, "Shared", "far away.pdf"), GREEN, "From afar")
    write_png(os.path.join(remote_root, "Shared", "near.png"), VIOLET)
    loupe = os.path.join(target, "spatiloupe")
    logs = {}

    def run(name, opened, steps, during=None, environment=None, runtime=None, slow=0, mesh_is="patched"):
        """One run of the room with SpatiLoupe on `opened`; `during(env, profile)` runs beside it.
        `runtime`, the runtime folder to run in, is the caller's to remove; else one of its own.
        `slow`: seconds SpatiMesh waits before it starts, as a slow machine's would. `mesh_is`:
        "patched", SpatiMesh as it is; "unpatched", one too old for places (before patch 0035) --
        the SDK's example as one (`--old-spatimesh`, bound to spatiand_xr_v1 12), which opens a
        window of its own; "nothing", a SpatiMesh that opens nothing at all."""
        # Short: the room's sockets go in it, and a socket's path has to fit.
        own_runtime = runtime is None
        runtime = runtime or tempfile.mkdtemp(prefix="spl2-", dir="/tmp")
        mesh = os.path.join(work, f"mesh-{name}")
        chromium = os.path.join(work, f"chromium-{name}")
        os.makedirs(chromium)
        runner = os.path.join(chromium, "run.sh")
        start = (f'exec "{CHROMIUM}" --no-sandbox --no-first-run --no-default-browser-check '
                 f'--password-store=basic --user-data-dir="{mesh}" "$@" 2>>"{os.path.join(out, name + "-chromium.log")}"\n')
        if mesh_is == "unpatched":
            # What a SpatiMesh too old for places is to the room: a window of SpatiMesh's
            # application id, naming no place, from a client bound to spatiand_xr_v1 before 13.
            start = (f'SPATIAND_XR_MOST=12 exec "{os.path.join(target, "examples", "embed")}" --old-spatimesh '
                     f'2>>"{os.path.join(out, name + "-chromium.log")}"\n')
        open(runner, "w").write("#!/bin/sh\n" + (f"sleep {slow}\n" + start if mesh_is != "nothing" else "exit 0\n"))
        os.chmod(runner, 0o755)
        launcher = os.path.join(work, f"{name}.sh")
        open(launcher, "w").write(f'#!/bin/sh\nexec "{loupe}" "{opened}"\n')
        os.chmod(launcher, 0o755)
        env = dict(os.environ)
        for drop in ("DISPLAY", "WAYLAND_DISPLAY", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"):
            env.pop(drop, None)
        env.update({
            "XDG_RUNTIME_DIR": runtime,
            "HOME": home,
            "XDG_CONFIG_HOME": os.path.join(home, ".config"),
            "XDG_CACHE_HOME": os.path.join(home, ".cache"),
            "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
            "EGL_PLATFORM": "surfaceless",
            "SPATIAND_BACKEND": "snapshot",
            "SPATIAND_SNAPSHOT": os.path.join(out, f"{name}.png"),
            "SPATIAND_CLIENT": launcher,
            "SPATIAND_CHROMIUM": chromium,
            "SPATIAND_HMD": "null",
            "SPATIAND_INPUT": "off",
            "SPATIAND_STEPS": ";".join(steps),
            "RUST_LOG": "info",
        })
        env.update(environment or {})
        apps = os.path.join(home, ".local", "share", "spatiand-apps.log")
        if os.path.exists(apps):
            os.remove(apps)
        log = os.path.join(out, f"{name}-spatiand.log")
        with open(log, "w") as f:
            room = subprocess.Popen([os.path.join(target, "spatiand")], env=env, stdout=f, stderr=subprocess.STDOUT)
            if during:
                threading.Thread(target=during, args=(env, mesh), daemon=True).start()
            try:
                room.wait(timeout=300)
            except subprocess.TimeoutExpired:
                room.kill()
                room.wait()
        # SpatiMesh, if anything of it is left: this run's only, by its profile.
        subprocess.run(["pkill", "-f", f"[u]ser-data-dir={mesh}"])
        said = open(apps, errors="replace").read() if os.path.exists(apps) else ""
        if os.path.exists(apps):
            shutil.copy(apps, os.path.join(out, f"{name}-apps.log"))
        logs[name] = (open(log, errors="replace").read(), said)
        if own_runtime:
            shutil.rmtree(runtime, ignore_errors=True)
        return logs[name]

    # Only some of the cases, by name (SPL_CASES=shown,keys): all, unless said.
    only = [c for c in os.environ.get("SPL_CASES", "").split(",") if c]

    def want(name):
        return not only or name.split("-")[0] in only

    # The window is SpatiLoupe's, 1440 by 900: SpatiMesh's viewer under the bar, from 120 down.
    right, left, wait_mesh = "key:106", "key:105", "wait:14"
    if want("shown"):
        run("shown", first, [wait_mesh, "window:SpatiLoupe", "describe", "wait:1", f"dump:{os.path.join(out, 'shown-loupe.png')}"])
    if want("picture"):
        run("picture", first, [wait_mesh, "window:SpatiLoupe", right, "wait:6"])
    if want("back"):
        run("back", first, [wait_mesh, "window:SpatiLoupe", right, "wait:4", left, wait_mesh])
    if want("next"):
        run("next", first, [wait_mesh, "window:SpatiLoupe", right, "wait:3", right, wait_mesh])
    if want("resized"):
        run("resized", first, [wait_mesh, "window:SpatiLoupe", "size:1000,700", "wait:5"])
    # The info pane open beside the viewer, which leaves its column.
    if want("info"):
        run("info", first, [wait_mesh, "window:SpatiLoupe", "key:23", "wait:4"])
    # Back to the folder a moment after SpatiMesh was asked, before its window has come (SpatiMesh
    # slowed by 8 seconds: the steps begin once SpatiLoupe has drawn, which may be 3 seconds after
    # it asked): the place let go of at once, and SpatiMesh's window, naming it when it comes, asked
    # by the room to close -- no window of its own in the room.
    if want("early"):
        run("early", first, ["wait:1.5", "window:SpatiLoupe", "key:1", "wait:18"], slow=8)
    # A SpatiMesh too old for places (the installed ones until the 159 build): it opens the PDF in a
    # window of its own and never comes into the place; the room tells the place so as that window
    # arrives, and SpatiLoupe lets the place go, leaves that window be and goes back to the folder,
    # which says so. And one that opens nothing at all: after 60 s the view says so, Open again.
    if want("apart"):
        run("apart", first, ["wait:8", "window:SpatiLoupe", "describe", "wait:1", f"dump:{os.path.join(out, 'apart-loupe.png')}"], mesh_is="unpatched")
        run("apart-nothing", first, ["wait:64", "window:SpatiLoupe", "describe", "wait:1", f"dump:{os.path.join(out, 'apart-nothing-loupe.png')}"], mesh_is="nothing")
    # A press in the viewer's page, then the arrow key: SpatiMesh's. A press on the bar, then the
    # arrow key: SpatiLoupe's, to the picture.
    if want("keys"):
        run("keys-mesh", first, [wait_mesh, "window:SpatiLoupe", "click:0.5,0.6", "wait:2", right, "wait:4"])
        run("keys-back", first, [wait_mesh, "window:SpatiLoupe", "click:0.5,0.6", "wait:2", "click:0.5,0.06", "wait:1", right, "wait:6"])
    if want("tools"):
        # Where the viewer's annotate button is, as fractions of SpatiLoupe's window: in the
        # viewer's bar, under the infobar Chromium shows for --no-sandbox (Chromium 157's layout).
        run("tools", first, [wait_mesh, "window:SpatiLoupe", f"click:{os.environ.get('SPL_ANNOTATE', '0.573,0.232')}", "wait:4"])

    # SpatiMesh ended under it, 18 seconds in (killed): said, with Open again; pressed, shown again.
    def end_mesh(env, mesh):
        time.sleep(18)
        subprocess.run(["pkill", "-f", f"[u]ser-data-dir={mesh}"])
    if want("closed"):
        closed_steps = [wait_mesh, "window:SpatiLoupe", "wait:8", f"dump:{os.path.join(out, 'closed-loupe.png')}",
                        "click:0.5,0.585", wait_mesh]
        run("closed", first, closed_steps, during=end_mesh)

    if storage and want("remote"):
        remote_case(run, storage, work, home, remote_root, out)

    failures = []

    def check(what, ok):
        print(("ok    " if ok else "FAIL  ") + what)
        if not ok:
            failures.append(what)

    def counted(name, colour, by=34):
        path = os.path.join(out, f"{name}.png")
        if not os.path.exists(path):
            return -1
        w, h, rows = e2e.read_png(path)
        return e2e.count(rows, 0, 0, w, h, lambda p: e2e.near(p, colour, by))

    def said(name):
        return logs.get(name, ("", ""))[1]

    def room_said(name):
        return logs.get(name, ("", ""))[0]

    asked = "SpatiMesh asked to show a PDF in a place in the window"
    if "shown" in logs:
        orange, violet = counted("shown", ORANGE), counted("shown", VIOLET)
        check("shown: SpatiMesh asked for the PDF in a place in SpatiLoupe's window", asked in said("shown"))
        check("shown: a window went inside the place", "a window is inside another's place now" in room_said("shown"))
        check(f"shown: the room's picture has the PDF's page ({orange} orange pixels), not the picture ({violet})", orange > 20000 and violet < 2000)
        check("shown: the room's speech is told a PDF shown by SpatiMesh, by name",
              "The PDF 1 report #1.pdf, shown by SpatiMesh's PDF viewer inside SpatiLoupe's window." in room_said("shown"))
        check("shown: no window of SpatiMesh's own in the room", "it stays a window of its own" not in room_said("shown"))

    if "picture" in logs:
        orange, violet = counted("picture", ORANGE), counted("picture", VIOLET)
        check(f"picture: the picture shown ({violet} violet pixels), the PDF gone ({orange} orange)", violet > 20000 and orange < 2000)
        check("picture: SpatiMesh's window asked to close with its place, not gone on its own", "SpatiMesh's window has gone" not in said("picture"))
        check("picture: no window of SpatiMesh's own left in the room", "it stays a window of its own" not in room_said("picture"))

    if "back" in logs:
        orange, violet = counted("back", ORANGE), counted("back", VIOLET)
        check(f"back: the PDF shown again ({orange} orange pixels; {violet} violet)", orange > 20000 and violet < 2000)
        check("back: in a new place", said("back").count(asked) == 2)

    if "next" in logs:
        green, orange = counted("next", GREEN), counted("next", ORANGE)
        check(f"next: the other PDF shown ({green} green pixels), the first gone ({orange} orange)", green > 20000 and orange < 2000)
        check("next: no window of SpatiMesh's own left in the room", "it stays a window of its own" not in room_said("next"))

    if "resized" in logs and "shown" in logs:
        orange_small, orange_big = counted("resized", ORANGE), counted("shown", ORANGE)
        check(f"resized: the PDF still shown, smaller ({orange_small} orange pixels, {orange_big} before)", 5000 < orange_small < orange_big)

    if "info" in logs:
        orange, info = counted("info", ORANGE), os.path.join(out, "info.png")
        check(f"info: the PDF still shown beside the info pane ({orange} orange pixels)", orange > 15000)
        if os.path.exists(info) and "shown" in logs:
            # The viewer's right edge: the band's rightmost orange pixel, in from where it was.
            def rightmost(name):
                w, h, rows = e2e.read_png(os.path.join(out, f"{name}.png"))
                xs = [x for y in range(0, h, 6) for x in range(0, w, 3) if e2e.near(rows[y][x], ORANGE, 34)]
                return max(xs) if xs else 0
            narrow, wide = rightmost("info"), rightmost("shown")
            check(f"info: the viewer leaves the pane's column (the page's right edge at {narrow}, was {wide})", narrow < wide - 150)

    if "early" in logs:
        check("early: SpatiMesh asked, then the folder gone back to before its window came", asked in said("early"))
        check("early: its window, naming the place let go of, asked by the room to close as it came",
              "a window came for a place taken away before it did; it is asked to close" in room_said("early"))
        check("early: no window of SpatiMesh's own in the room", "it stays a window of its own" not in room_said("early"))
        orange = counted("early", ORANGE)
        check(f"early: the room shows the folder, no PDF ({orange} orange pixels)", orange < 2000)

    if "apart" in logs:
        check("apart: the room told the place a SpatiMesh too old for places opened it in a window of its own",
              "too old for places" in room_said("apart") and "SpatiMesh will not come into its place" in said("apart"))
        check("apart: no window went into the place", "a window is inside another's place now" not in room_said("apart"))
        check("apart: the old SpatiMesh's window of its own left be (not asked to close)", "asked to close" not in room_said("apart"))
        green = counted("apart", GREEN)
        check(f"apart: its window of its own in the room ({green} of its pixels)", green > 20000)
        check("apart: SpatiLoupe is back at the folder, saying SpatiMesh opened it in a window of its own",
              "describe: said" in room_said("apart") and "SpatiMesh opened 1 report #1.pdf in a window of its own" in room_said("apart"))
        check("apart: a SpatiMesh that opens nothing: after 60 s the place let go of, and said, with Open again",
              "did not come into its place in 60 s" in said("apart-nothing") and "SpatiMesh has not shown 1 report #1.pdf here." in room_said("apart-nothing"))
        check(f"apart: and nothing of the PDF in the room ({counted('apart-nothing', ORANGE)} orange pixels)", counted("apart-nothing", ORANGE) < 2000)

    if "keys-mesh" in logs:
        orange, violet = counted("keys-mesh", ORANGE), counted("keys-mesh", VIOLET)
        check(f"keys: after a press in the viewer the arrow key is SpatiMesh's: the PDF stays ({orange} orange, {violet} violet)", orange > 20000 and violet < 2000)
        orange, violet = counted("keys-back", ORANGE), counted("keys-back", VIOLET)
        check(f"keys: after a press on the bar the arrow key is SpatiLoupe's: the picture ({violet} violet, {orange} orange)", violet > 20000 and orange < 2000)

    if "tools" in logs:
        # The pen's swatches, Chromium's own Blue 2, Red 2, Green 2 and Yellow 2, in its side panel.
        swatches = [counted("tools", c, 24) for c in [(66, 133, 244), (234, 67, 53), (52, 168, 83), (251, 188, 4)]]
        check(f"tools: a press on the viewer's annotate button opened its tools, Chromium's pen colours there ({swatches} pixels)", all(n > 150 for n in swatches))

    if "closed" in logs:
        orange = counted("closed", ORANGE)
        check("closed: SpatiLoupe heard SpatiMesh's window go", "SpatiMesh's window has gone from its place" in said("closed"))
        check(f"closed: Open again showed it again ({orange} orange pixels)", orange > 20000 and said("closed").count(asked) == 2)

    if "remote" in logs:
        green = counted("remote", GREEN)
        copies = os.path.join(home, ".cache", "spatiloupe", "remote")
        copied = [os.path.join(d, f) for d, _, fs in os.walk(copies) for f in fs if f == "far away.pdf"]
        check(f"remote: copied into SpatiLoupe's cache ({copied})", len(copied) == 1 and open(copied[0], "rb").read() == open(os.path.join(remote_root, "Shared", "far away.pdf"), "rb").read())
        check(f"remote: shown from its copy ({green} green pixels)", green > 20000)

    time.sleep(1)
    left_over = subprocess.run(["pgrep", "-f", work], capture_output=True, text=True).stdout.split()
    check(f"nothing of the test is left running ({left_over})", not left_over)
    shutil.rmtree(work, ignore_errors=True)
    print(f"pictures and logs in {out}")
    sys.exit(1 if failures else 0)


def remote_case(run, storage, work, home, remote_root, out):
    """A remote's PDF: a storage service of the test's own -- its own runtime folder, remotes file
    and token, nobody else's service or remotes -- with an rclone alias remote over a scratch
    folder; SpatiLoupe opened on scratch:Shared/far away.pdf, in the same runtime folder, where it
    finds that service."""
    bindir = os.path.join(work, "bin")
    os.makedirs(bindir)
    shutil.copy(storage, os.path.join(bindir, "spatiand-storage"))
    conf = os.path.join(work, "remotes.conf")
    open(conf, "w").write(f"[scratch]\ntype = alias\nremote = {remote_root}\n")
    runtime = tempfile.mkdtemp(prefix="spl2-", dir="/tmp")
    env = dict(os.environ)
    for drop in ("DISPLAY", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"):
        env.pop(drop, None)
    env.update({"XDG_RUNTIME_DIR": runtime, "HOME": home, "SPATIAND_STORAGE_CONFIG": conf})
    service = subprocess.Popen([os.path.join(bindir, "spatiand-storage")], env=env, stdin=subprocess.DEVNULL,
                               stdout=open(os.path.join(out, "storage.log"), "w"), stderr=subprocess.STDOUT, start_new_session=True)
    try:
        for _ in range(100):
            if os.path.exists(os.path.join(runtime, "spatiand-storage.json")):
                break
            time.sleep(0.1)
        run("remote", "scratch:Shared/far away.pdf", ["wait:18", "window:SpatiLoupe", "describe", "wait:1"],
            environment={"SPATIAND_STORAGE_CONFIG": conf}, runtime=runtime)
    finally:
        service.terminate()
        try:
            service.wait(timeout=10)
        except subprocess.TimeoutExpired:
            service.kill()
        shutil.rmtree(runtime, ignore_errors=True)


if __name__ == "__main__":
    main()
