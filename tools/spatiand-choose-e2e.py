#!/usr/bin/env python3
"""The file chooser with the storage service's remotes, end to end, in the room's snapshot backend
with no GPU, no display and no glasses.

A storage service of the test's own (its own runtime folder, remotes file and token: nobody else's
service or remotes are touched), with a scratch rclone `alias` remote over a scratch folder, a
WebDAV remote at an address that never answers (192.0.2.1, kept for documentation) and a Google
Drive with no sign-in (listed, never opened); and spatiand-choose run by the room as SpatiMesh's
Chromium runs it, what it writes kept:

    open       the remotes under the places; one that does not answer, "Loading..."; a remote
               folder; a file found in it and Entered, fetched at two megabytes a second while
               the window says how far it has come; its path written, the file byte for byte
    stop       begun in a remote (--dir scratch:Docs), a fetch stopped part way by Escape: back
               to choosing, rclone's job over, nothing of it left; a second Escape a cancel
    folder     a remote's folder typed, gone to and chosen: fetched whole
    save       begun in a remote (--dir scratch:Docs), a name there already: Replace asked, then
               the outbox's path written; a browser's download written beside it in parts and
               renamed; the sender moves it on, the remote has it byte for byte, and the
               sender's notice is on the room's card
    unreached  no storage service to be had: one quiet line, and this machine's files as ever

    tools/spatiand-choose-e2e.py <spatiand-storage> [target/debug] [out-folder]

The chooser's output is read through a pipe to its end, as Chromium reads it, so a sender holding
it would show as a chooser that never finished. The room is `spatiand` in the target folder, or
SPATIAND_ROOM_BIN. The pictures (`dump:`, the window's own) are left in the out folder.
"""

import base64
import importlib.util
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.request

sys.dont_write_bytecode = True
here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("e2e", os.path.join(here, "e2e_png.py"))
e2e = importlib.util.module_from_spec(spec)
spec.loader.exec_module(e2e)

TEAL = (22, 230, 192)


def main():
    storage = os.path.abspath(sys.argv[1])
    target = os.path.abspath(sys.argv[2] if len(sys.argv) > 2 else "target/debug")
    out = os.path.abspath(sys.argv[3] if len(sys.argv) > 3 else "target/spatiand-choose-e2e")
    room = os.environ.get("SPATIAND_ROOM_BIN", os.path.join(target, "spatiand"))
    chooser = os.path.join(target, "spatiand-choose")
    os.makedirs(out, exist_ok=True)
    work = tempfile.mkdtemp(prefix="spatiand-choose-e2e-")
    # Short: the room's sockets go in it, and a socket's path has to fit.
    runtime = tempfile.mkdtemp(prefix="spc-", dir="/tmp")
    home = os.path.join(work, "home")
    documents = os.path.join(home, "Documents")
    os.makedirs(documents)
    os.makedirs(os.path.join(home, "Downloads"))
    open(os.path.join(documents, "letter.txt"), "w").write("hello")
    open(os.path.join(documents, "plan.pdf"), "w").write("%PDF plan")
    remote = os.path.join(work, "remote")
    os.makedirs(os.path.join(remote, "Docs", "Old"))
    os.makedirs(os.path.join(remote, "album", "inner"))
    open(os.path.join(remote, "Docs", "report.pdf"), "wb").write(b"%PDF the old report")
    open(os.path.join(remote, "Docs", "notes.txt"), "wb").write(b"some notes")
    film = os.urandom(24_000_000)
    open(os.path.join(remote, "Docs", "film.bin"), "wb").write(film)
    open(os.path.join(remote, "album", "one.png"), "wb").write(b"one")
    open(os.path.join(remote, "album", "inner", "two.png"), "wb").write(b"two")
    config = os.path.join(work, "config")
    os.makedirs(config)
    conf = os.path.join(config, "remotes.conf")
    open(conf, "w").write(
        f"[scratch]\ntype = alias\nremote = {remote}\n\n"
        "[nas]\ntype = webdav\nurl = http://192.0.2.1/\nvendor = other\n\n"
        "[photos]\ntype = drive\n"
    )
    bindir = os.path.join(work, "bin")
    os.makedirs(bindir)
    shutil.copy(storage, os.path.join(bindir, "spatiand-storage"))
    env = dict(os.environ)
    for name in ("DISPLAY", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"):
        env.pop(name, None)
    env.update({
        "XDG_RUNTIME_DIR": runtime,
        "HOME": home,
        "XDG_CONFIG_HOME": os.path.join(home, ".config"),
        "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
        "SPATIAND_STORAGE_CONFIG": conf,
        "SPATIAND_APPS_DIR": bindir,
        "SPATIAND_HMD": "null",
        "SPATIAND_INPUT": "off",
        "EGL_PLATFORM": "surfaceless",
        "SPATIAND_BACKEND": "snapshot",
        "RUST_LOG": "info",
    })
    service = subprocess.Popen([os.path.join(bindir, "spatiand-storage")], env=env, stdin=subprocess.DEVNULL,
                               stdout=open(os.path.join(out, "storage.log"), "w"), stderr=subprocess.STDOUT,
                               start_new_session=True)
    place_file = os.path.join(runtime, "spatiand-storage.json")
    for _ in range(100):
        if os.path.exists(place_file):
            break
        time.sleep(0.1)
    place = json.load(open(place_file))

    def rc(method, body):
        request = urllib.request.Request(f"http://127.0.0.1:{place['port']}/rc/{method}", data=json.dumps(body).encode(), method="POST")
        request.add_header("Authorization", "Basic " + base64.b64encode(f"spatiand:{place['token']}".encode()).decode())
        request.add_header("Content-Type", "application/json")
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        return json.load(opener.open(request, timeout=30))

    def run(name, args, steps, environment=env, during=None):
        """The room with the chooser in it, until its steps are done; `during`, if given, is
        called while the room is still up."""
        launcher = os.path.join(work, f"{name}.sh")
        quoted = " ".join("'" + a.replace("'", "'\\''") + "'" for a in args)
        # Its output read through a pipe to its end, as Chromium reads it (GetAppOutput): the
        # exit is written only once every holder of the pipe has let go -- a sender holding it
        # would keep it from being written until the sender was done.
        open(launcher, "w").write(
            "#!/bin/sh\n"
            f"( '{chooser}' {quoted} 2> '{out}/chooser-{name}.log'; echo $? > '{work}/{name}.status' ) | cat > '{work}/{name}.out'\n"
            f"mv '{work}/{name}.status' '{work}/{name}.code'\n")
        os.chmod(launcher, 0o755)
        run_env = dict(environment)
        run_env.update({
            "SPATIAND_CLIENT": launcher,
            "SPATIAND_STEPS": ";".join(steps),
            "SPATIAND_SNAPSHOT": os.path.join(out, f"room-{name}.png"),
        })
        with open(os.path.join(out, f"spatiand-{name}.log"), "w") as log:
            room_run = subprocess.Popen([room], env=run_env, stdout=log, stderr=subprocess.STDOUT)
            try:
                if during:
                    during()
                room_run.wait(timeout=300)
            finally:
                if room_run.poll() is None:
                    room_run.kill()
                    room_run.wait()
        # The chooser is done by the time the room is; a moment for the launcher to say how.
        for _ in range(50):
            if os.path.exists(os.path.join(work, f"{name}.code")):
                break
            time.sleep(0.1)
        code = open(os.path.join(work, f"{name}.code")).read().strip() if os.path.exists(os.path.join(work, f"{name}.code")) else "none"
        lines = open(os.path.join(work, f"{name}.out")).read().splitlines() if os.path.exists(os.path.join(work, f"{name}.out")) else []
        return code, lines

    failures = []

    def check(what, ok):
        print(("ok    " if ok else "FAIL  ") + what)
        if not ok:
            failures.append(what)

    def shot(name):
        return os.path.join(out, f"{name}.png")

    # The chooser's own folders are in the cache folder on the disk (its own::base), here the test's.
    own_root = os.path.join(home, ".cache", "spatiand-choose")
    fetched_root = os.path.join(own_root, "fetched")
    outbox_root = os.path.join(own_root, "outbox")
    try:
        # Two megabytes a second, after rclone's first four at once: film.bin's 24 are fetched
        # slowly enough to be seen going.
        rc("core/bwlimit", {"rate": "2M"})
        code, lines = run("open", ["--mode", "open", "--dir", documents], [
            "wait:3", "window:Choose a file", f"dump:{shot('places')}",
            "type:nas:", "key:28", "wait:1.5", f"dump:{shot('loading')}",
            "type:scratch:Docs", "key:28", "wait:2", f"dump:{shot('remote-folder')}",
            "type:film", "wait:0.5", f"dump:{shot('found')}",
            "key:28", "wait:3", f"dump:{shot('fetching')}", "wait:12",
        ])
        check(f"open: chosen (exit {code})", code == "0")
        check(f"open: one path, in the chooser's fetched folder ({lines})", len(lines) == 1 and lines[0].startswith(fetched_root + "/"))
        check("open: the file fetched byte for byte", len(lines) == 1 and os.path.exists(lines[0]) and open(lines[0], "rb").read() == film)
        if os.path.exists(shot("fetching")):
            w, h, rows = e2e.read_png(shot("fetching"))
            filled = e2e.count(rows, 0, 0, w, h, lambda p: e2e.near(p, TEAL, 40))
            check(f"open: how far it has come, as a bar part filled ({filled} teal pixels)", 2000 < filled < w * 10)
        else:
            check("open: photographed while fetching", False)

        # Stopped part way: back to choosing, the job over, nothing of it left.
        before = sorted(os.listdir(fetched_root))
        code, lines = run("stop", ["--mode", "open", "--dir", "scratch:Docs"], [
            "wait:3", "window:Choose a file", "type:film", "key:28", "wait:3", f"dump:{shot('fetch-stopping')}",
            "key:1", "wait:2", f"dump:{shot('fetch-stopped')}", "key:1", "wait:1",
        ])
        check(f"stop: Escape stops the fetch, and a second is a cancel (exit {code}, nothing written)", code == "1" and lines == [])
        check(f"stop: nothing of it left in the fetched folder ({sorted(os.listdir(fetched_root))})", sorted(os.listdir(fetched_root)) == before)
        # Every call to rclone is a job of its own, the unanswered listing of the first run's
        # included: what is asked is whether the file is still being moved.
        moving = [t.get("name") for t in rc("core/stats", {}).get("transferring", [])]
        check(f"stop: rclone no longer fetching it ({moving})", "film.bin" not in moving)
        rc("core/bwlimit", {"rate": "off"})

        code, lines = run("folder", ["--mode", "folder", "--dir", home], [
            "wait:3", "window:Choose a folder", "type:scratch:album", "key:28", "wait:1.5", f"dump:{shot('remote-album')}",
            "key:28", "wait:3",
        ])
        check(f"folder: chosen (exit {code})", code == "0")
        got = lines[0] if len(lines) == 1 else ""
        check(f"folder: fetched whole into the chooser's own folder ({lines})", got.startswith(fetched_root + "/") and got.endswith("/album"))
        check("folder: what is in it, and in what is in it", os.path.exists(os.path.join(got, "inner", "two.png")) and open(os.path.join(got, "one.png"), "rb").read() == b"one")

        new = b"%PDF the new report, " + bytes(range(256)) * 4000
        sent = os.path.join(remote, "Docs", "report.pdf")
        seen = {"saved": "", "senders": []}

        def browser_saves():
            # With the room still up, as it is when a browser saves: once the chooser is done,
            # the browser writes its download, the sender moves it on and says so on the card.
            for _ in range(300):
                if os.path.exists(os.path.join(work, "save.code")):
                    break
                time.sleep(0.1)
            out_file = os.path.join(work, "save.out")
            chosen = open(out_file).read().splitlines() if os.path.exists(out_file) else []
            saved = chosen[0] if len(chosen) == 1 else ""
            if not saved:
                return
            seen["saved"] = saved
            seen["senders"] = subprocess.run(["pgrep", "-f", "--", f"--send {saved}"], capture_output=True, text=True).stdout.split()
            # The browser: its download written beside the name in parts, then renamed to it.
            with open(saved + ".crdownload", "wb") as part:
                for i in range(0, len(new), len(new) // 3 + 1):
                    part.write(new[i:i + len(new) // 3 + 1])
                    part.flush()
                    time.sleep(1)
            os.rename(saved + ".crdownload", saved)
            for _ in range(300):
                if open(sent, "rb").read() == new and not os.path.exists(os.path.dirname(saved)):
                    break
                time.sleep(0.1)

        # The room stays up for the sender's notice: twelve seconds is the file written in three
        # and sent in one or two, with room to spare.
        code, lines = run("save", ["--mode", "save", "--dir", "scratch:Docs", "--name", "report.pdf"], [
            "wait:3", "window:Save as", "key:28", "wait:1.5", f"dump:{shot('replace')}", "key:28", "wait:2", "wait:12",
        ], during=browser_saves)
        check(f"save: chosen (exit {code})", code == "0")
        saved = lines[0] if len(lines) == 1 else ""
        check(f"save: a path in the outbox, by its name ({lines})", saved.startswith(outbox_root + "/") and saved.endswith("/report.pdf"))
        check(f"save: its sender running ({seen['senders']})", len(seen["senders"]) == 1)
        check("save: the remote has it, byte for byte", open(sent, "rb").read() == new)
        check("save: its folder in the outbox taken away", saved and not os.path.exists(os.path.dirname(saved)))
        log = open(os.path.join(own_root, "send.log")).read() if os.path.exists(os.path.join(own_root, "send.log")) else ""
        check("save: the sender's log says so", "sent report.pdf to scratch:Docs/report.pdf" in log)
        time.sleep(0.5)
        check("save: the sender gone", not subprocess.run(["pgrep", "-f", "--", f"--send {saved}"], capture_output=True).stdout)
        room_log = open(os.path.join(out, "spatiand-save.log")).read()
        check("save: the sender's notice on the room's card", "a notice from Files on the card" in room_log)
        # Where it went, for SpatiMesh's downloads list: sent/<outbox folder>.json.
        record_path = os.path.join(own_root, "sent", os.path.basename(os.path.dirname(saved)) + ".json") if saved else ""
        record = json.load(open(record_path)) if record_path and os.path.exists(record_path) else {}
        check(f"save: a record of where it went, for the browser ({record})", record.get("in") == "scratch:Docs" and record.get("to") == "scratch:Docs/report.pdf")

        # No service to be had: a runtime folder of its own with none in it, and none installed.
        lonely = tempfile.mkdtemp(prefix="spc-", dir="/tmp")
        nothing = os.path.join(work, "no-programs")
        os.makedirs(nothing)
        alone = dict(env)
        alone.update({"XDG_RUNTIME_DIR": lonely, "SPATIAND_APPS_DIR": nothing})
        code, lines = run("unreached", ["--mode", "open", "--dir", documents], [
            "wait:3", "window:Choose a file", f"dump:{shot('unreached')}", "key:1", "wait:1",
        ], alone)
        check(f"unreached: Escape is a cancel (exit {code}, nothing written)", code == "1" and lines == [])
        shutil.rmtree(lonely, ignore_errors=True)
    finally:
        os.killpg(service.pid, signal.SIGTERM)
        service.wait(timeout=10)
        left = subprocess.run(["pgrep", "-af", work], capture_output=True, text=True).stdout.strip()
        left += subprocess.run(["pgrep", "-af", runtime], capture_output=True, text=True).stdout.strip()
        check(f"nothing of the test's left running ({left or 'none'})", not left)
        shutil.rmtree(work, ignore_errors=True)
        shutil.rmtree(runtime, ignore_errors=True)
    print(f"pictures in {out}")
    if failures:
        print(f"{len(failures)} check(s) failed")
        sys.exit(1)


if __name__ == "__main__":
    main()
