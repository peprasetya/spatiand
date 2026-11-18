#!/usr/bin/env python3
"""Removing a holder gives the vault new keys: three software holders over one folder, driven
over their sockets (docs/spatikey.md, "Removing a holder makes new keys").

    cargo build -p spatikey-core --example soft-service
    tools/spatikey-rekey-rig.py target/debug/examples/soft-service <an empty scratch folder>

Never against a real vault: the holders' keys are in files in the clear and every touch is a yes.
The three share one synced folder, so what the synchroniser would bring later is put there by
hand: an entry file the phone sealed under the old key, arriving after the keys changed.
"""
import base64, hashlib, json, os, shutil, signal, socket, struct, subprocess, sys, time

soft, root = sys.argv[1], sys.argv[2]
if os.path.exists(root) and os.listdir(root):
    sys.exit(f"{root} is not empty")
os.makedirs(root, exist_ok=True)
apps = os.path.join(root, "apps/spatikey")
running = {}


def fp(name):
    return hashlib.sha256(name.encode()).hexdigest()


def start(name, kind=None, env=None, wait=True):
    sock = os.path.join(root, f"{name}.sock")
    if os.path.exists(sock):
        os.remove(sock)
    args = [soft, root, sock, name] + ([kind] if kind else [])
    log = open(os.path.join(root, f"{name}.log"), "a")
    proc = subprocess.Popen(args, stdout=log, stderr=log, env={**os.environ, **(env or {})})
    if not wait:
        return proc
    for _ in range(600):
        if os.path.exists(sock):
            break
        time.sleep(0.1)
    else:
        sys.exit(f"{name} did not start")
    running[name] = (proc, sock)
    return proc


def stop(name):
    proc, _ = running.pop(name)
    proc.send_signal(signal.SIGTERM)
    proc.wait()


class Conn:
    def __init__(self, name):
        self.s = socket.socket(socket.AF_UNIX)
        self.s.connect(running[name][1])
        self.n = 0

    def ask(self, op, **fields):
        self.n += 1
        # The envelope's number first, then the request, whose own "id" (an entry's) is a second
        # key of that name: as the Rust client writes it.
        body = ('{"id":%d,' % self.n + json.dumps({"op": op, **fields})[1:]).encode()
        self.s.sendall(struct.pack(">I", len(body)) + body)
        size = struct.unpack(">I", self.read(4))[0]
        reply = json.loads(self.read(size))
        return reply.get("ok"), reply.get("err")

    def read(self, n):
        data = b""
        while len(data) < n:
            more = self.s.recv(n - len(data))
            if not more:
                sys.exit("a holder went away")
            data += more
        return data

    def ok(self, op, **fields):
        ok, err = self.ask(op, **fields)
        if err:
            sys.exit(f"FAILED: {op} refused: {err}")
        return ok


checks = 0


import atexit
atexit.register(lambda: [stop(name) for name in list(running)])


def must(held, what):
    global checks
    checks += 1
    print(("ok:     " if held else "FAILED: ") + what)
    if not held:
        for name in list(running):
            stop(name)
        sys.exit(1)


def entry(title, password):
    return {"kind": "login", "title": title, "sites": ["example.com"], "username": "paulus", "password": password}


def sealed_under(entry_id):
    return json.load(open(os.path.join(apps, "entries", entry_id + ".json"))).get("k", "")


def titles(conn, session):
    return sorted(e["title"] for e in conn.ok("list", session=session)["entries"])


def add(holder, session, name):
    code = Conn(name).ok("holder_request", name=name)["code"]
    holder.ok("holder_approve", session=session, fp=fp(name), code=code)


# The Mac makes the vault; the phone and the tablet are added to it.
start("mac")
recovery = None
for line in open(os.path.join(root, "mac.log")):
    pass
mac = Conn("mac")
session = mac.ok("open")["session"]
recovery = mac.ok("recovery_new", session=session)["recovery_code"]
start("phone", "android")
start("tablet", "android")
add(mac, session, "phone")
add(mac, session, "tablet")
ids = {t: mac.ok("put", session=session, entry=entry(t, t + "-secret"))["id"] for t in ["GitHub", "Bank", "Steam"]}
phone = Conn("phone")
phone_session = phone.ok("open")["session"]
must(titles(phone, phone_session) == ["Bank", "GitHub", "Steam"], "three holders open one vault")

# What the tablet could keep: its half (its key and its wrap), and the vault as it is now.
must(len(Conn("tablet").ok("open")["entries"]) == 3, "the tablet among them")
kept = os.path.join(root, "kept-by-tablet")
shutil.copytree(os.path.join(root, "local-tablet"), os.path.join(kept, "local-tablet"))
tablet_half = json.load(open(os.path.join(kept, "local-tablet/spatikey/touch-id.json")))

# The phone changes Bank under the old key; that file is kept aside and the folder put back as
# it was, as though the change had not reached the Mac yet.
bank = os.path.join(apps, "entries", ids["Bank"] + ".json")
aside = os.path.join(root, "bank-before")
shutil.copy2(bank, aside)
time.sleep(0.01)
phone.ok("put", session=phone_session, entry={**entry("Bank, changed on the phone", "Bank-new"), "id": ids["Bank"]})
late = os.path.join(root, "bank-from-the-phone")
shutil.copy2(bank, late)
shutil.copy2(aside, bank)

# The Mac removes the tablet.
done = mac.ok("forget_device", session=session, fp=fp("tablet"))
must(done.get("rekeyed") is True and not done.get("recovery_code"), "the tablet is removed, the vault has new keys, the recovery code stays")
chain = sorted(os.listdir(os.path.join(apps, "chain")))
must(len(chain) == 1, "one file is the change of keys")
first = chain[0][:-5]
must(all(sealed_under(i) == first for i in ids.values()), "every entry is sealed under the new key")
must(titles(mac, session) == ["Bank", "GitHub", "Steam"], "the Mac's open SpatiKey goes on")
tomb = json.load(open(os.path.join(apps, "devices", fp("tablet") + ".json")))
must(tomb.get("removed") is True and not tomb.get("wrapped") and not tomb.get("public"), "the tablet's record is a tombstone")
everything = "".join(open(os.path.join(d, f)).read() for d, _, fs in os.walk(apps) for f in fs)
must(tablet_half["public"] not in everything and tablet_half["wrapped"] not in everything, "nothing in the synced folder is wrapped to the tablet's key")

# The tablet as a thief has it: started again from what it kept, over the vault as it now is.
stop("tablet")
shutil.rmtree(os.path.join(root, "local-tablet"))
shutil.copytree(os.path.join(kept, "local-tablet"), os.path.join(root, "local-tablet"))
start("tablet", "android")
tablet = Conn("tablet")
ok, err = tablet.ask("open")
must(ok is None and err is not None, f"the tablet's old key and old wrap open nothing ({err['code'] if err else ''})")
must(tablet.ok("status")["opens_here"] is False, "and it is no holder")

# The phone's change, sealed under the old key, arrives now: newer than the Mac's sealing again.
shutil.copy2(late, bank)
os.utime(bank, None)
must(sealed_under(ids["Bank"]) == "", "the phone's change arrives sealed under the old key")
ok, err = phone.ask("list", session=phone_session)
must(err is not None and err["code"] == "bad_session", "the phone's open SpatiKey is told to open again")
phone_session = phone.ok("open")["session"]
ALL = ["Bank, changed on the phone", "GitHub", "Steam"]
must(titles(phone, phone_session) == ALL, "the phone follows the new keys with its own touch, and its change was not lost")
must(sealed_under(ids["Bank"]) == first, "which is sealed under the new key now")
must(titles(mac, session) == ALL, "the Mac reads it too")

# The recovery code, the same one, makes a new device a holder of the vault as it now is.
start("spare")
Conn("spare").ok("recover", code=recovery)
spare = Conn("spare")
must(titles(spare, spare.ok("open")["session"]) == ALL, "the recovery code opens everything")

# The Mac dies in the middle of removing the spare: the one file written, nothing else.
stop("mac")
broken = start("mac", env={"SOFT_BREAK_REKEY": "spare"}, wait=False)
must(broken.wait() == 9, "a holder stopped right after the change of keys was written")
chain = sorted(f[:-5] for f in os.listdir(os.path.join(apps, "chain")))
must(len(chain) == 2, "the second change of keys is there")
second = [c for c in chain if c != first][0]
must(all(sealed_under(i) == first for i in ids.values()), "no entry was sealed again before it stopped")
must(json.load(open(os.path.join(apps, "devices", fp("spare") + ".json"))).get("removed") is not True, "and no tombstone written")
# The phone opens the vault next, and finishes what the Mac began.
phone_session = Conn("phone").ok("open")["session"]
must(all(sealed_under(i) == second for i in ids.values()), "whoever opens the vault next seals every entry under the newest key")
must(json.load(open(os.path.join(apps, "devices", fp("spare") + ".json"))).get("removed") is True, "and writes the tombstone")
start("mac")
mac = Conn("mac")
session = mac.ok("open")["session"]
must(titles(mac, session) == ALL, "the Mac, started again, follows its own change")
ok, err = Conn("spare").ask("open")
must(ok is None, "the spare opens nothing any more")
start("spare2")
Conn("spare2").ok("recover", code=recovery)
must(len(Conn("spare2").ok("open")["entries"]) == 3, "the recovery code still opens everything")

for name in list(running):
    stop(name)
print(f"all {checks} held")
