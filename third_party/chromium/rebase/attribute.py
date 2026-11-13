#!/usr/bin/env python3
"""attribute.py <fixes.diff>: in the export worktree (cwd), give each file's fix to the patch that last touched
that file in the stack, as 'fixup! <subject>' commits (one per patch), for a following autosquash. Files no
patch touches are left unapplied and listed, for a person to place."""
import os, re, subprocess, sys, collections
BASE = os.environ["CARRY_BASE"]  # the new version's tag
def sh(*a, **k): return subprocess.run(a, capture_output=True, **k)
t = open(sys.argv[1], "rb").read()
parts = [p for p in re.split(rb"(?m)^(?=diff --git )", t) if p.startswith(b"diff --git ")]
groups = collections.defaultdict(list); orphans = []
for part in parts:
    name = part.split(b"\n", 1)[0].split(b" b/", 1)[1].decode()
    subj = sh("git", "log", "-1", "--format=%s", f"{BASE}..HEAD", "--", name).stdout.decode().strip()
    if subj.startswith("fixup! "): subj = subj[7:]
    if not subj:
        for line in open(os.environ["CARRY_STATE"] + "/orphans.map"):
            pre, _, to = line.rstrip().partition(chr(9))
            if pre and name.startswith(pre): subj = to
    (groups[subj] if subj else orphans).append((name, part))
for subj, files in groups.items():
    bad = []
    for name, part in files:
        r = sh("git", "apply", "--index", "-", input=part)
        if r.returncode: bad.append((name, r.stderr.decode().strip().split("\n")[-1][:120]))
    sh("git", "commit", "-q", "-m", "fixup! " + subj)
    print(f"{len(files) - len(bad):3d} file(s) -> {subj[:70]}")
    for n, w in bad: print("    FAILED", n, w)
print("ORPHANS (no patch touches the file):")
for name, _ in orphans: print("   ", name)
open(sys.argv[1] + ".orphans", "wb").write(b"".join(p for _, p in orphans))
