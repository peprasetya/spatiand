#!/usr/bin/env python3
"""SpatiMesh's own menus and pages, end to end, with no GPU, no display and no glasses.

As tools/spatimesh-e2e.py: the room's compositor (SPATIAND_BACKEND=snapshot) runs SpatiMesh
with WebKitGTK showing a page from a server started here, SPATIAND_STEPS drives it, and each
step's photograph is checked:

    ctx-link      a right click on a link: SpatiMesh's menu for it
    ctx-page      a right click on the page itself: its own menu
    menu-flyout   the main menu, and its Search engine submenu beside it
    suggest       letters typed in the address bar: the page visited, suggested and finished
    find          Control-F: the find bar, what it found counted
    bookmark      Control-D: the page bookmarked, its bookmark's menu
    bar           Control-Shift-B: the bookmarks bar, the bookmark on it
    history       Control-H: the history page
    settings      the settings page
    extensions    the extensions page, and the files to choose an unpacked one from (picker)
    tab-menu      a right click on a tab
    reopened      a tab closed (Control-W) and opened again (Control-Shift-T)

and then SpatiMesh is started again on the same profile, as after it stopped:

    restored      the tabs it had open, back
    start         a new tab: the start page, with the sites visited most and the bookmark
    downloaded    a link's "Save link as": the file in Downloads, the toolbar's downloads button
    downloads     Control-J: the downloads page

    tools/spatimesh-ui-e2e.py [target/debug] [out-folder]
"""

import http.server
import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
import threading

# Its helpers are tools/spatimesh-e2e.py's, loaded without leaving a cache in tools/.
sys.dont_write_bytecode = True
here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("e2e", os.path.join(here, "spatimesh-e2e.py"))
e2e = importlib.util.module_from_spec(spec)
spec.loader.exec_module(e2e)

PAGE = """<!doctype html><html><head><title>SpatiMesh menus</title><style>
body{margin:0;height:1000px;background:#f4f6fb;font:24px sans-serif}
#a{position:absolute;left:80px;top:120px;width:400px;height:40px;display:block}
#p{position:absolute;left:80px;top:220px;width:700px}
</style></head><body>
<a id=a href="/second.html">A link to a second page</a>
<p id=p>A needle in the page, another needle, and a third needle.</p>
</body></html>"""
FILES = """<!doctype html><html><head><title>Files</title><style>body{margin:0;font:24px sans-serif}
#f{position:absolute;left:80px;top:120px;width:400px;height:40px;display:block}</style></head>
<body><a id=f href="/thing.zip">A file to save</a></body></html>"""
THING = bytes(range(256)) * 400
SECOND = """<!doctype html><html><head><title>Second page</title></head><body style="margin:0;background:#8f6bf2"></body></html>"""

WIDTH, HEIGHT = 1600, 1000
ROW = 64  # a row and the gap after it, in a menu


def at(x, y):
    return f"{x / WIDTH:.4f},{y / HEIGHT:.4f}"


def page_at(css_x, css_y):
    return at(10 + css_x * 1.25, 122 + css_y * 1.25)


def main():
    target = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/debug")
    out = os.path.abspath(sys.argv[2] if len(sys.argv) > 2 else "target/spatimesh-ui-e2e")
    os.makedirs(out, exist_ok=True)
    work = tempfile.mkdtemp(prefix="spatimesh-ui-e2e-")
    www = os.path.join(work, "www")
    os.makedirs(www)
    open(os.path.join(www, "index.html"), "w").write(PAGE)
    open(os.path.join(www, "second.html"), "w").write(SECOND)
    open(os.path.join(www, "files.html"), "w").write(FILES)
    open(os.path.join(www, "thing.zip"), "wb").write(THING)
    # Something to find in the file picker: an unpacked extension's folder.
    os.makedirs(os.path.join(work, "my-extension"))
    open(os.path.join(work, "my-extension", "manifest.json"), "w").write('{"manifest_version":3,"name":"Mine","version":"1.0"}')

    class Quiet(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=www, **kwargs)

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Quiet)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    port = server.server_address[1]

    names = ["ctx-link", "ctx-page", "menu-flyout", "suggest", "find", "bookmark", "bar", "history", "settings", "extensions", "picker", "tab-menu", "reopened", "restored", "start", "downloaded", "downloads"]
    shots = {name: os.path.join(out, f"{name}.png") for name in names}
    menu_button = at(1562, 88)
    address = at(800, 88)
    # The main menu: its lines from y = 138, the ninth (Search engine) of a web page's.
    search_engine_line = at(1300, 138 + 8 * ROW + 30)
    steps = [
        "wait:3",
        f"type:localhost:{port}/",
        "key:28",
        "wait:6",
        # The link visited, and back: something in the history.
        f"click:{page_at(200, 140)}",
        "wait:3",
        "key:alt+105",
        "wait:3",
        f"rclick:{page_at(200, 140)}",
        "wait:2",
        f"dump:{shots['ctx-link']}",
        "key:1",
        f"rclick:{page_at(900, 600)}",
        "wait:2",
        f"dump:{shots['ctx-page']}",
        "key:1",
        f"click:{menu_button}",
        "wait:1",
        f"move:{search_engine_line}",
        "wait:1",
        f"dump:{shots['menu-flyout']}",
        "key:1",
        "key:1",
        "wait:1",
        f"click:{address}",
        "type:loc",
        "wait:2",
        f"dump:{shots['suggest']}",
        "key:1",
        "key:1",
        "key:ctrl+33",
        "type:needle",
        "wait:2",
        f"dump:{shots['find']}",
        "key:1",
        "key:ctrl+32",
        "wait:1",
        f"dump:{shots['bookmark']}",
        "key:1",
        "key:ctrl+shift+48",
        "wait:1",
        f"dump:{shots['bar']}",
        "key:ctrl+shift+48",
        "key:ctrl+35",
        "wait:2",
        f"dump:{shots['history']}",
        f"click:{address}",
        "type:spatimesh:settings",
        "key:28",
        "wait:2",
        f"dump:{shots['settings']}",
        f"click:{address}",
        "type:spatimesh:extensions",
        "key:28",
        "wait:2",
        f"dump:{shots['extensions']}",
        # Down twice from Chrome Web Store: Load an unpacked folder.
        "key:108",
        "key:108",
        "key:28",
        "wait:1",
        f"type:{work}",
        "key:28",
        "wait:1",
        f"dump:{shots['picker']}",
        "key:1",
        f"rclick:{at(175, 33)}",
        "wait:1",
        f"dump:{shots['tab-menu']}",
        "key:1",
        "key:ctrl+17",
        "wait:1",
        "key:ctrl+shift+20",
        "wait:2",
        f"dump:{shots['reopened']}",
        # What is open is written down within ten seconds of changing.
        "wait:12",
    ]
    again = [
        "wait:5",
        f"dump:{shots['restored']}",
        "key:ctrl+20",
        "wait:2",
        f"dump:{shots['start']}",
        f"type:localhost:{port}/files.html",
        "key:28",
        "wait:4",
        f"rclick:{page_at(200, 140)}",
        "wait:2",
        # The link's menu: its sixth line, Save link as.
        f"click:{at(10 + 200 * 1.25 + 200, 122 + 140 * 1.25 + 2 + 12 + 5 * ROW + 30)}",
        "wait:4",
        f"dump:{shots['downloaded']}",
        "key:ctrl+36",
        "wait:2",
        f"dump:{shots['downloads']}",
    ]
    runtime = os.path.join(work, "run")
    os.makedirs(runtime, mode=0o700)

    def run(steps, name):
        env = dict(os.environ)
        env.update({
            "XDG_RUNTIME_DIR": runtime,
            "HOME": work,
            "SPATIMESH_HOME": os.path.join(work, "spatimesh"),
            "SPATIMESH_ENGINE": "webkitgtk",
            "EGL_PLATFORM": "surfaceless",
            "SPATIAND_BACKEND": "snapshot",
            "SPATIAND_SNAPSHOT": os.path.join(out, f"room-{name}.png"),
            "SPATIAND_CLIENT": os.path.join(target, "spatimesh"),
            "SPATIAND_STEPS": ";".join(steps),
            "RUST_LOG": "info",
        })
        env.pop("DISPLAY", None)
        log = open(os.path.join(out, f"spatiand-{name}.log"), "w")
        subprocess.run([os.path.join(target, "spatiand")], env=env, stdout=log, stderr=subprocess.STDOUT, timeout=420)
        apps_log = os.path.join(work, ".local/share/spatiand-apps.log")
        if os.path.exists(apps_log):
            shutil.copy(apps_log, os.path.join(out, f"apps-{name}.log"))

    run(steps, "first")
    run(again, "again")
    server.shutdown()
    saved = os.path.join(work, "Downloads", "thing.zip")
    check_saved = os.path.exists(saved) and open(saved, "rb").read() == THING

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
        w, h, rows = e2e.read_png(path)
        check(f"{name}: the whole window ({w}x{h})", (w, h) == (WIDTH, HEIGHT))
        pictures[name] = rows

    def dark_glass(name, x0, y0, x1, y1, least=4000):
        """A pane of SpatiMesh's (dark glass with light words) over the page's light ground."""
        if name not in pictures:
            return
        rows = pictures[name]
        dark = e2e.count(rows, x0, y0, x1, y1, lambda p: max(p) < 60)
        light = e2e.count(rows, x0, y0, x1, y1, lambda p: min(p) > 190)
        check(f"{name}: SpatiMesh's pane ({dark} dark, {light} light pixels)", dark > least and light > 100)

    # The link's menu opens at the pointer, below and right of it.
    x, y = 10 + 200 * 1.25, 122 + 140 * 1.25
    dark_glass("ctx-link", x + 20, y + 20, x + 440, y + 300)
    x, y = 10 + 900 * 1.25, 122 + 600 * 1.25
    dark_glass("ctx-page", max(0, x - 500), 200, min(WIDTH, x + 400), HEIGHT - 20)
    dark_glass("menu-flyout", 1040, 140, 1580, 900)
    dark_glass("menu-flyout", 510, 400, 1000, 800)
    dark_glass("suggest", 300, 120, 1300, 260)
    dark_glass("find", 950, 140, 1580, 190, least=2000)
    dark_glass("bookmark", 1000, 130, 1500, 400)
    dark_glass("tab-menu", 180, 40, 590, 500)
    dark_glass("picker", 200, 160, 1400, 900)

    if "reopened" in pictures and "restored" in pictures:
        # Two tabs' titles in light ink on the bar: the closed one back, and after the restart.
        for name in ("reopened", "restored"):
            rows = pictures[name]
            ink = e2e.count(rows, 360, 18, 660, 48, lambda p: min(p) > 170)
            check(f"{name}: a second tab on the bar ({ink} light pixels)", ink > 150)
    check("downloaded: the file is in ~/Downloads, whole", check_saved)
    if "start" in pictures:
        rows = pictures["start"]
        # The start page's most visited tiles: colour, where the page is otherwise dark ground.
        coloured = e2e.count(rows, 60, 300, 1500, 450, lambda p: max(p) - min(p) > 40)
        check(f"start: the most visited sites ({coloured} coloured pixels)", coloured > 3000)

    shutil.rmtree(work, ignore_errors=True)
    print(f"pictures in {out}")
    if failures:
        print(f"{len(failures)} check(s) failed")
        sys.exit(1)


if __name__ == "__main__":
    main()
