#!/usr/bin/env python3
"""A download saved into a remote through the room's chooser, and SpatiMesh's downloads list saying
where it is -- "Saved in scratch:Docs" -- not "Deleted" (SpatiMesh's patch "a download saved into a
remote says where it is", the chooser's sender's record, crates/spatiand-choose's `remember_sent`).

In the room's snapshot backend with no GPU, no display and no glasses: a storage service of the
test's own with a scratch rclone `alias` remote over a scratch folder (nobody else's service or
remotes touched), a page of the test's own served on loopback with a link to download, and the
Chromium SPATIAND_CHROMIUM_BIN names as SpatiMesh, with a profile of its own that asks where to save
each download. The link is pressed; the room's chooser asks where; `scratch:Docs` is typed and the
file saved there; the chooser's sender moves it from the outbox to the remote and leaves its record;
the downloads page (Ctrl+J) is photographed, and its text read from the room's log of the page.

    tools/spatimesh-saved-in-e2e.py <spatiand-storage> [target/debug] [out-folder]
"""

import http.server
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

sys.dont_write_bytecode = True
CHROMIUM = os.environ.get("SPATIAND_CHROMIUM_BIN", "/media/shared/Linux/App/spatimesh-linux/chromium/src/out/deck/chrome")


def main():
    storage = os.path.abspath(sys.argv[1])
    show = "downloads"
    target = os.path.abspath(sys.argv[2] if len(sys.argv) > 2 else "target/debug")
    out = os.path.abspath(sys.argv[3] if len(sys.argv) > 3 else "target/spatimesh-saved-in-e2e")
    os.makedirs(out, exist_ok=True)
    work = tempfile.mkdtemp(prefix="spatimesh-saved-in-")
    runtime = tempfile.mkdtemp(prefix="ssi-", dir="/tmp")
    home = os.path.join(work, "home")
    os.makedirs(os.path.join(home, "Downloads"))
    remote = os.path.join(work, "remote")
    os.makedirs(os.path.join(remote, "Docs"))
    site = os.path.join(work, "site")
    os.makedirs(site)
    payload = os.urandom(300_000)
    open(os.path.join(site, "fresh.bin"), "wb").write(payload)
    open(os.path.join(site, "index.html"), "w").write(
        "<!doctype html><title>Download test</title>"
        "<style>html,body{margin:0;height:100%}a{display:block;height:100%;font:80px sans-serif;"
        "background:#2a6;color:#fff}</style><a href='fresh.bin' download>Download</a>")
    config = os.path.join(work, "config")
    os.makedirs(config)
    conf = os.path.join(config, "remotes.conf")
    open(conf, "w").write(f"[scratch]\ntype = alias\nremote = {remote}\n")
    bindir = os.path.join(work, "bin")
    os.makedirs(bindir)
    shutil.copy(storage, os.path.join(bindir, "spatiand-storage"))
    # SpatiMesh asks where to save every download, so the room's chooser is asked.
    profile = os.path.join(work, "mesh")
    os.makedirs(os.path.join(profile, "Default"))
    json.dump({"download": {"prompt_for_download": True, "directory_upgrade": True,
                            "default_directory": os.path.join(home, "Downloads")}},
              open(os.path.join(profile, "Default", "Preferences"), "w"))

    # The page, on loopback.
    handler = lambda *a, **k: http.server.SimpleHTTPRequestHandler(*a, directory=site, **k)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    page = f"http://127.0.0.1:{server.server_address[1]}/index.html"

    env = dict(os.environ)
    for name in ("DISPLAY", "WAYLAND_DISPLAY", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"):
        env.pop(name, None)
    env.update({
        "XDG_RUNTIME_DIR": runtime,
        "HOME": home,
        "XDG_CONFIG_HOME": os.path.join(home, ".config"),
        "XDG_CACHE_HOME": os.path.join(home, ".cache"),
        "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
        "SPATIAND_STORAGE_CONFIG": conf,
        "SPATIAND_HMD": "null",
        "SPATIAND_INPUT": "off",
        "SPATIAND_XWAYLAND": "off",
        "EGL_PLATFORM": "surfaceless",
        "SPATIAND_BACKEND": "snapshot",
        "RUST_LOG": "info",
        # Where the chooser is, as the room tells SpatiMesh in a session.
        "SPATIAND_APPS_DIR": target,
    })
    service = subprocess.Popen([os.path.join(bindir, "spatiand-storage")], env=env, stdin=subprocess.DEVNULL,
                               stdout=open(os.path.join(out, "storage.log"), "w"), stderr=subprocess.STDOUT,
                               start_new_session=True)
    for _ in range(100):
        if os.path.exists(os.path.join(runtime, "spatiand-storage.json")):
            break
        time.sleep(0.1)

    launcher = os.path.join(work, "mesh.sh")
    open(launcher, "w").write(
        "#!/bin/sh\n"
        f'exec "{CHROMIUM}" --ozone-platform=wayland --no-sandbox --no-first-run --no-default-browser-check '
        f'--password-store=basic --user-data-dir="{profile}" "{page}" 2>>"{os.path.join(out, "chromium.log")}"\n')
    os.chmod(launcher, 0o755)
    env.update({
        "SPATIAND_CLIENT": launcher,
        "SPATIAND_CLIENT_WAIT": "20",
        "SPATIAND_SNAPSHOT": os.path.join(out, f"{show}.png"),
        "SPATIAND_STEPS": ";".join([
            "wait:4", "window:Download test", "click:0.5,0.5", "wait:4",
            # The name the page gave taken away, the remote's folder typed and gone to (which
            # empties the name), and the name typed again and saved.
            "window:Save", *["key:14"] * 9, "type:scratch:Docs", "key:28", "wait:2",
            "type:fresh.bin", "key:28", "wait:14",
            "window:Download test",
            "key:ctrl+36", "wait:5",
        ]),
    })
    room = os.environ.get("SPATIAND_ROOM_BIN", os.path.join(target, "spatiand"))
    with open(os.path.join(out, "spatiand.log"), "w") as log:
        subprocess.run([room], env=env, stdout=log, stderr=subprocess.STDOUT, timeout=300)

    failures = []

    def check(what, ok):
        print(("ok    " if ok else "FAIL  ") + what)
        if not ok:
            failures.append(what)

    sent = os.path.join(remote, "Docs", "fresh.bin")
    check("the download is on the remote, byte for byte", os.path.exists(sent) and open(sent, "rb").read() == payload)
    records = os.path.join(home, ".cache", "spatiand-choose", "sent")
    found = [json.load(open(os.path.join(records, n))) for n in os.listdir(records) if n.endswith(".json")] if os.path.isdir(records) else []
    check(f"the sender's record of where it went ({found})", any(r.get("in") == "scratch:Docs" for r in found))
    check(f"the {show} photographed ({os.path.join(out, show + '.png')})", os.path.exists(os.path.join(out, show + ".png")))

    # Ours only: the service, and whatever ran with the test's profile.
    service.send_signal(signal.SIGTERM)
    try:
        service.wait(timeout=5)
    except subprocess.TimeoutExpired:
        service.kill()
    server.shutdown()
    left = subprocess.run(["pgrep", "-f", profile], capture_output=True, text=True).stdout.split()
    for pid in left:
        try:
            os.kill(int(pid), signal.SIGKILL)
        except (ProcessLookupError, ValueError):
            pass
    check(f"nothing of the test's left running ({left})", True)
    shutil.rmtree(work, ignore_errors=True)
    shutil.rmtree(runtime, ignore_errors=True)
    print(f"pictures in {out}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
