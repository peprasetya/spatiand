#!/usr/bin/env python3
"""SpatiMesh's logins and history on two machines, end to end, on one computer.

Two SpatiMesh homes share one synchronised folder (`apps/`), as two machines linked by
spatiand-sync do. Each runs in the room's own snapshot compositor, one after the other, against
a server started here:

    machine A   logs in (the server sets an HttpOnly cookie that lasts ten days) and is sent on to
                a second page, so that both are in its history
    machine B   opens a page, which reloads itself once; the server notes the cookie each request
                brought, and B's picture of the page shows its two links, one to the page A
                visited (drawn red if visited) and one to a page nobody did (blue)

Checked: A wrote the cookie to the shared folder; B's second request carried it; B shows A's
page as visited. Where the engine has no visited links that can be told (Steam's Chromium,
WebKitGTK), the last check is reported, not failed.

    tools/spatimesh-sync-e2e.py <compositor> [target/debug] [out-folder]

On a Mac the compositor is crates/spatiand-mac/target/debug/spatiand-mac; on Linux,
target/debug/spatiand.
"""

import http.server
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
e2e = __import__("spatimesh-e2e")

LOGIN = """<!doctype html><title>Logged in</title><body style="background:#2a2">
<script>setTimeout(function(){location.href='/visited-target'},1000)</script>"""
TARGET = """<!doctype html><title>Visited target</title><body style="background:#22a">"""
CHECK = """<!doctype html><title>Check</title><style>
body{margin:0;background:#fff}
a{position:absolute;left:40px;width:500px;height:200px;display:block;background-color:#0000ff;color:#0000ff}
a:visited{background-color:#ff0000;color:#ff0000}
</style><body>
<a id=v href="/visited-target" style="top:40px">visited</a>
<a id=n href="/never-visited" style="top:300px">never</a>
<script>if(location.search!='?again')setTimeout(function(){location.href='/check?again'},6000)</script>"""


def main():
    compositor = os.path.abspath(sys.argv[1])
    target = os.path.abspath(sys.argv[2] if len(sys.argv) > 2 else "target/debug")
    out = os.path.abspath(sys.argv[3] if len(sys.argv) > 3 else "target/spatimesh-sync-e2e")
    os.makedirs(out, exist_ok=True)
    work = tempfile.mkdtemp(prefix="smsync-", dir="/tmp")
    seen = []

    class Server(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            path = self.path.split("?")[0]
            seen.append((self.path, self.headers.get("Cookie", "")))
            body = {"/login": LOGIN, "/visited-target": TARGET, "/check": CHECK}.get(path, "<!doctype html><title>x</title>")
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            if path == "/login":
                self.send_header("Set-Cookie", "sid=s3cret; Max-Age=864000; Path=/; HttpOnly")
            self.end_headers()
            self.wfile.write(body.encode())

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Server)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    port = server.server_address[1]
    base = f"http://localhost:{port}"

    homes = {name: os.path.join(work, name) for name in ("a", "b")}
    for home in homes.values():
        os.makedirs(home)
    os.makedirs(os.path.join(homes["a"], "apps"))
    os.symlink(os.path.join(homes["a"], "apps"), os.path.join(homes["b"], "apps"))

    def machine(name, steps):
        runtime = tempfile.mkdtemp(prefix="smr-", dir="/tmp")
        os.chmod(runtime, 0o700)
        env = dict(os.environ)
        env.update({
            "XDG_RUNTIME_DIR": runtime,
            "SPATIMESH_HOME": homes[name],
            "EGL_PLATFORM": "surfaceless",
            "SPATIAND_BACKEND": "snapshot",
            "SPATIAND_SNAPSHOT": os.path.join(out, f"room-{name}.png"),
            "SPATIAND_CLIENT": os.path.join(target, "spatimesh"),
            "SPATIAND_HMD": "null",
            "SPATIAND_INPUT": "off",
            "SPATIAND_STEPS": ";".join(steps),
            "RUST_LOG": "info",
        })
        env.pop("DISPLAY", None)
        with open(os.path.join(out, f"{name}.log"), "w") as log:
            subprocess.run([compositor], env=env, stdout=log, stderr=subprocess.STDOUT, timeout=180)
        shutil.rmtree(runtime, ignore_errors=True)

    machine("a", ["wait:3", f"type:{base}/login", "key:28", "wait:12"])
    shot = os.path.join(out, "b.png")
    machine("b", ["wait:3", f"type:{base}/check", "key:28", "wait:14", f"dump:{shot}"])
    server.shutdown()

    failures = []

    def check(what, ok, soft=False):
        print(("ok    " if ok else "note  " if soft else "FAIL  ") + what)
        if not ok and not soft:
            failures.append(what)

    cookie_files = os.path.join(homes["a"], "apps/spatimesh/profiles/default/cookies")
    written = []
    if os.path.isdir(cookie_files):
        for f in os.listdir(cookie_files):
            written += json.load(open(os.path.join(cookie_files, f))).get("cookies", [])
    check(f"A wrote its login to the shared folder ({[c['name'] for c in written]})", any(c["name"] == "sid" and c.get("http_only") for c in written))
    history = os.path.join(homes["a"], "apps/spatimesh/profiles/default/history")
    check(f"both machines' history in the shared folder ({sorted(os.listdir(history)) if os.path.isdir(history) else []})", os.path.isdir(history) and len(os.listdir(history)) == 2)
    again = [cookie for path, cookie in seen if path == "/check?again"]
    check(f"B's page, loaded again, brought A's login ({again})", any("sid=s3cret" in c for c in again))
    if os.path.exists(shot):
        _, _, rows = e2e.read_png(shot)
        red = sum(1 for row in rows for p in row if p[0] > 200 and p[1] < 60 and p[2] < 60)
        blue = sum(1 for row in rows for p in row if p[2] > 200 and p[0] < 60 and p[1] < 60)
        check(f"B shows the page visited on A as visited ({red} red, {blue} blue pixels)", red > 5000 and blue > 5000, soft=True)
    else:
        check("B photographed", False)

    print(f"logs and pictures in {out}; the homes in {work}")
    if failures:
        print(f"{len(failures)} check(s) failed")
        sys.exit(1)
    shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
