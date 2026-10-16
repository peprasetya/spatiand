#!/usr/bin/env python3
"""SpatiMesh end to end, with no GPU, no display and no glasses.

The room's own compositor (SPATIAND_BACKEND=snapshot, drawing in software where there is no
GPU) runs SpatiMesh as its client, with WebKitGTK showing a page from a server started here;
SPATIAND_STEPS types, clicks and photographs SpatiMesh's window after each step, and the
photographs are checked here, pixel by pixel:

    loaded    an address typed into SpatiMesh's bar loads the page
    typed     a field on the page clicked and typed into
    second    a link followed
    back      SpatiMesh's Back button
    select    the page's <select> opened as SpatiMesh's own list

    tools/spatimesh-e2e.py [target/debug] [out-folder]

The programs are the ones built in the folder given (spatiand, spatimesh, spatimesh-webkitgtk).
The photographs are left in out-folder (target/spatimesh-e2e) to be looked at.
"""

import http.server
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import threading
import zlib

# Everything placed exactly, so that where to click does not depend on the fonts installed.
PAGE = """<!doctype html><html><head><title>SpatiMesh test page</title><style>
body{margin:0;height:1000px;background:linear-gradient(135deg,#16e6c0,#37a6f7 50%,#8f6bf2) no-repeat}
#card{position:absolute;left:40px;top:40px;width:900px;height:500px;background:#fff;border-radius:20px}
#i{position:absolute;left:80px;top:200px;width:500px;height:40px;font:24px sans-serif;box-sizing:border-box}
#a{position:absolute;left:80px;top:300px;width:400px;height:40px;display:block;font:24px sans-serif}
#s{position:absolute;left:80px;top:380px;width:160px;height:36px}
</style></head><body><div id=card></div>
<input id=i oninput="document.title='typed: '+this.value">
<a id=a href="/second.html">A link to a second page</a>
<select id=s><option>Apple</option><option>Banana</option><option>Cherry</option></select>
</body></html>"""
SECOND = """<!doctype html><html><head><title>Second page</title></head>
<body style="margin:0;background:#8f6bf2"></body></html>"""

# SpatiMesh's window is 1600x1000; its page starts at (10, 122) and is drawn at 1.25x.
PAGE_X, PAGE_Y, ZOOM, WIDTH, HEIGHT = 10, 122, 1.25, 1600, 1000


def window_point(css_x, css_y):
    return PAGE_X + css_x * ZOOM, PAGE_Y + css_y * ZOOM


def fraction(css_x, css_y):
    x, y = window_point(css_x, css_y)
    return f"{x / WIDTH:.4f},{y / HEIGHT:.4f}"


def read_png(path):
    """A PNG's width, height and rows of (r, g, b): eight bits, RGB or RGBA, not interlaced."""
    data = open(path, "rb").read()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", f"{path} is not a PNG"
    pos, idat, width = 8, b"", None
    while pos < len(data):
        length, kind = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + length]
        if kind == b"IHDR":
            width, height, depth, colour, _, _, interlace = struct.unpack(">IIBBBBB", body)
            assert depth == 8 and colour in (2, 6) and interlace == 0, f"{path}: an unexpected PNG"
            channels = 4 if colour == 6 else 3
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    raw = zlib.decompress(idat)
    stride = width * channels
    rows, previous, i = [], bytearray(stride), 0
    for _ in range(height):
        kind, line = raw[i], bytearray(raw[i + 1:i + 1 + stride])
        i += 1 + stride
        for x in range(stride):
            left = line[x - channels] if x >= channels else 0
            up = previous[x]
            corner = previous[x - channels] if x >= channels else 0
            if kind == 1:
                line[x] = (line[x] + left) & 255
            elif kind == 2:
                line[x] = (line[x] + up) & 255
            elif kind == 3:
                line[x] = (line[x] + (left + up) // 2) & 255
            elif kind == 4:
                p = left + up - corner
                pa, pb, pc = abs(p - left), abs(p - up), abs(p - corner)
                line[x] = (line[x] + (left if pa <= pb and pa <= pc else up if pb <= pc else corner)) & 255
        rows.append([tuple(line[x * channels:x * channels + 3]) for x in range(width)])
        previous = line
    return width, height, rows


def near(a, b, by=12):
    return all(abs(x - y) <= by for x, y in zip(a, b))


def count(rows, x0, y0, x1, y1, test):
    return sum(1 for y in range(int(y0), int(y1)) for x in range(int(x0), int(x1)) if test(rows[y][x]))


def main():
    target = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/debug")
    out = os.path.abspath(sys.argv[2] if len(sys.argv) > 2 else "target/spatimesh-e2e")
    os.makedirs(out, exist_ok=True)
    work = tempfile.mkdtemp(prefix="spatimesh-e2e-")
    www = os.path.join(work, "www")
    os.makedirs(www)
    open(os.path.join(www, "index.html"), "w").write(PAGE)
    open(os.path.join(www, "second.html"), "w").write(SECOND)

    class Quiet(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=www, **kwargs)

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Quiet)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    port = server.server_address[1]

    shots = {name: os.path.join(out, f"{name}.png") for name in ("loaded", "typed", "second", "back", "select")}
    steps = ";".join([
        "wait:3",
        f"type:localhost:{port}",
        "key:28",
        "wait:8",
        f"dump:{shots['loaded']}",
        f"click:{fraction(330, 220)}",
        "wait:1",
        "type:hello mesh",
        "wait:2",
        f"dump:{shots['typed']}",
        f"click:{fraction(200, 320)}",
        "wait:5",
        f"dump:{shots['second']}",
        "click:0.0240,0.0880",
        "wait:5",
        f"dump:{shots['back']}",
        f"click:{fraction(160, 398)}",
        "wait:3",
        f"dump:{shots['select']}",
    ])
    runtime = os.path.join(work, "run")
    os.makedirs(runtime, mode=0o700)
    env = dict(os.environ)
    env.update({
        "XDG_RUNTIME_DIR": runtime,
        "HOME": work,
        "SPATIMESH_HOME": os.path.join(work, "spatimesh"),
        "SPATIMESH_ENGINE": "webkitgtk",
        "EGL_PLATFORM": "surfaceless",
        "SPATIAND_BACKEND": "snapshot",
        "SPATIAND_SNAPSHOT": os.path.join(out, "room.png"),
        "SPATIAND_CLIENT": os.path.join(target, "spatimesh"),
        "SPATIAND_STEPS": steps,
        "RUST_LOG": "info",
    })
    # Not the X display of whoever runs this: the engine is the compositor's client, by Wayland.
    env.pop("DISPLAY", None)
    log = open(os.path.join(out, "spatiand.log"), "w")
    subprocess.run([os.path.join(target, "spatiand")], env=env, stdout=log, stderr=subprocess.STDOUT, timeout=300)
    apps_log = os.path.join(work, ".local/share/spatiand-apps.log")
    if os.path.exists(apps_log):
        shutil.copy(apps_log, os.path.join(out, "apps.log"))
    server.shutdown()

    failures = []

    def check(what, ok):
        print(("ok    " if ok else "FAIL  ") + what)
        if not ok:
            failures.append(what)

    pictures = {}
    for name, path in shots.items():
        if not os.path.exists(path):
            check(f"{name}: photographed", False)
            continue
        w, h, rows = read_png(path)
        check(f"{name}: the whole window ({w}x{h})", (w, h) == (WIDTH, HEIGHT))
        pictures[name] = rows

    white = (255, 255, 255)
    if "loaded" in pictures:
        rows = pictures["loaded"]
        x, y = window_point(700, 480)
        check("loaded: the page's white card", near(rows[int(y)][int(x)], white))
        x, y = window_point(1150, 300)
        r, g, b = rows[int(y)][int(x)]
        check(f"loaded: the page's gradient beside it ({r},{g},{b})", b > 180 and r < 160)
    if "typed" in pictures:
        rows = pictures["typed"]
        x0, y0 = window_point(84, 204)
        x1, y1 = window_point(570, 236)
        dark = count(rows, x0, y0, x1, y1, lambda p: max(p) < 90)
        check(f"typed: text in the field ({dark} dark pixels)", dark > 150)
    if "second" in pictures:
        rows = pictures["second"]
        check(f"second: the second page ({rows[600][800]})", near(rows[600][800], (143, 107, 242)))
    if "back" in pictures:
        rows = pictures["back"]
        x, y = window_point(700, 480)
        check("back: the first page again", near(rows[int(y)][int(x)], white))
        x0, y0 = window_point(84, 204)
        x1, y1 = window_point(570, 236)
        dark = count(rows, x0, y0, x1, y1, lambda p: max(p) < 90)
        check(f"back: the field kept its text ({dark} dark pixels)", dark > 150)
    if "select" in pictures:
        rows = pictures["select"]
        # SpatiMesh's list opens below the <select>, on the page's white card: dark glass,
        # with light text on it.
        x0, y0 = window_point(80, 420)
        x1, y1 = window_point(300, 520)
        dark = count(rows, x0, y0, x1, y1, lambda p: max(p) < 60)
        bright = count(rows, x0, y0, x1, y1, lambda p: min(p) > 190)
        check(f"select: SpatiMesh's list over the page ({dark} dark, {bright} light pixels)", dark > 2000 and bright > 100)

    shutil.rmtree(work, ignore_errors=True)
    print(f"pictures in {out}")
    if failures:
        print(f"{len(failures)} check(s) failed")
        sys.exit(1)


if __name__ == "__main__":
    main()
