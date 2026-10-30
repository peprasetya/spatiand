#!/usr/bin/env python3
"""In the TEST COPY of the shared folder only: gives a profile a known PIN."""
import hashlib, json, os, sys
root, profile, secret = sys.argv[1], sys.argv[2], sys.argv[3]
assert root.startswith("/tmp/"), "the test copy only"
p = f"{root}/apps/spatimesh/profiles/{profile}/profile.json"
d = json.load(open(p))
salt = os.urandom(16).hex()
h = hashlib.sha256((salt + "\0" + secret).encode()).digest()
for _ in range(100000):
    h = hashlib.sha256(h).digest()
d["lock"] = {"kind": "pin", "salt": salt, "hash": h.hex()}
json.dump(d, open(p, "w"))
print("set", p)
