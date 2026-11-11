#!/usr/bin/env python3
"""Test profiles in the rig's copy of Spatiand's folder (SPATIAND_APPS_CONFIG=/tmp/smtest/apps
stands for ~/.config/spatiand, so the shared part is apps/ inside it), as another machine would
have left them: the default one named, and three more -- the last one locked with the PIN 2468.

    seed.py            (on littleotter, before go.sh)
"""

import hashlib
import json
import os

ROOT = "/tmp/smtest/apps/apps/spatimesh/profiles"
PROFILES = {
    "default": ("First@Test", [236, 206, 60], 0),
    "aaaa000000000001": ("Second", [143, 107, 242], 1792408296486),
    "aaaa000000000002": ("Third", [22, 230, 192], 1792227231651),
    "aaaa000000000003": ("Locked", [120, 200, 90], 1792898839974),
}

for ident, (name, colour, created) in PROFILES.items():
    os.makedirs(f"{ROOT}/{ident}", exist_ok=True)
    profile = {"name": name, "colour": colour, "created": created}
    if name == "Locked":
        salt = "0011223344556677"
        # LockHash (chrome/browser/spatiand/profiles_sync.cc): SHA-256 of the salt, a zero byte
        # and the secret, then 100000 times over
        digest = hashlib.sha256(salt.encode() + b"\0" + b"2468").digest()
        for _ in range(100000):
            digest = hashlib.sha256(digest).digest()
        profile["lock"] = {"kind": "pin", "salt": salt, "hash": digest.hex()}
    with open(f"{ROOT}/{ident}/profile.json", "w") as f:
        json.dump(profile, f, indent=2)
print("seeded", ", ".join(name for name, _, _ in PROFILES.values()))
