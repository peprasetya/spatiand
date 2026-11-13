#!/usr/bin/env python3
"""Resolve the conflicts a rule can: run in the worktree after a stopped port157.sh.
 - a file no Linux-UI build compiles (Android's): upstream's side as it is;
 - both sides added at one place (empty base): both, upstream's first;
 - the patch only added lines before or after a block upstream changed: upstream's block and the added lines.
Prints what is left for a person."""
import subprocess, re, sys
def sh(*a): return subprocess.run(a, capture_output=True, text=True).stdout
ANDROID = re.compile(r"(^|/)android/|_android\.(cc|h|mm)$|(^|/)android_[^/]*\.(cc|h)$|\.java$|/java/|^chrome/android/|^android_webview/|_win\.(cc|h)$|(^|/)win/|^ios/")
left = []
for f in [l for l in sh("git", "diff", "--name-only", "--diff-filter=U").split("\n") if l]:
    if ANDROID.search(f):
        r = subprocess.run(["git", "checkout", "--ours", "--", f], capture_output=True, text=True)
        if r.returncode != 0:  # deleted upstream
            subprocess.run(["git", "rm", "-q", "--cached", "-f", "--", f]); subprocess.run(["rm", "-f", f])
        else: subprocess.run(["git", "add", "--", f])
        print("upstream's (not built):", f); continue
    try: text = open(f, encoding="utf-8").read()
    except Exception: left.append((f, "unreadable or deleted")); continue
    out = []; ok = True; n = 0; hard = 0
    lines = text.split("\n"); i = 0
    while i < len(lines):
        if lines[i].startswith("<<<<<<< "):
            O = []; B = []; T = []; cur = O; i += 1
            while not lines[i].startswith(">>>>>>> "):
                if lines[i].startswith("||||||| "): cur = B
                elif lines[i] == "=======": cur = T
                else: cur.append(lines[i])
                i += 1
            n += 1
            if not B: res = O + T
            elif T == B: res = O
            elif O == B: res = T
            elif O == T: res = O
            elif len(T) > len(B) and T[:len(B)] == B: res = O + T[len(B):]
            elif len(T) > len(B) and T[-len(B):] == B: res = T[:len(T) - len(B)] + O
            else:
                res = None
            if res is None:
                ok = False; hard += 1
                out += ["<<<<<<< ours"] + O + ["||||||| base"] + B + ["======="] + T + [">>>>>>> theirs"]
            else: out += res
        else: out.append(lines[i])
        i += 1
    open(f, "w", encoding="utf-8").write("\n".join(out))
    if ok:
        subprocess.run(["git", "add", "--", f]); print(f"resolved by rule ({n}):", f)
    else: left.append((f, f"{hard} of {n} hunks need a person"))
for f, why in left: print("LEFT:", f, "--", why)
sys.exit(1 if left else 0)
