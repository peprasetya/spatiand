#!/usr/bin/env python3
"""applyeach.py <patch file>: apply a large diff file by file, three-way, into the index of the worktree in the
current folder, so that one file that cannot be applied does not hold back the rest. Lists conflicts and failures."""
import re, subprocess, sys
t = open(sys.argv[1], "rb").read()
parts = re.split(rb"(?m)^(?=diff --git )", t)
ok = 0; conflicts = []; failed = []
for part in parts:
    if not part.startswith(b"diff --git "): continue
    name = part.split(b"\n", 1)[0].split(b" b/", 1)[1].decode()
    r = subprocess.run(["git", "apply", "--3way", "--index", "-"], input=part, capture_output=True)
    if r.returncode == 0: ok += 1
    elif name in subprocess.run(["git", "diff", "--name-only", "--diff-filter=U"], capture_output=True, text=True).stdout.split("\n"): conflicts.append(name)
    else: failed.append((name, r.stderr.decode(errors="replace").strip().split("\n")[-1][:150]))
print(ok, "applied;", len(conflicts), "conflict;", len(failed), "failed")
for c in conflicts: print("CONFLICT", c)
for f, why in failed: print("FAILED", f, "--", why)
sys.exit(1 if conflicts or failed else 0)
