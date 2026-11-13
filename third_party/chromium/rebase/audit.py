#!/usr/bin/env python3
# For each patch N: files of the 153 commit (minus submodule paths) not touched by carried commit N nor by any
# "fixup! <subject>" commit; each must be logged for N in resolve.log (original "CONFLICT at N" section, or a
# MANUAL-R/REVIEW-R N line), or be an Android/Windows-only path. Otherwise: is the patch's change already in the
# tree (reverse-apply check)? Else UNACCOUNTED.
import subprocess, sys, re, os
S = os.path.dirname(os.path.abspath(__file__)); W = "/media/shared/Linux/App/spatimesh-linux/port157"
sys.path.insert(0, S); from patchlog import patch_logs
def sh(*a, inp=None): return subprocess.run(a, capture_output=True, cwd=W, input=inp)
def out(*a): return sh(*a).stdout.decode()
SUBS = ("v8/", "third_party/skia/", "third_party/devtools-frontend/src/", "third_party/boringssl/src/", "third_party/perfetto/")
ANDROID = re.compile(r"(^|/)android/|_android\.(cc|h|mm)$|(^|/)android_[^/]*\.(cc|h)$|\.java$|/java/|^chrome/android/|^android_webview/|_win\.(cc|h)$|(^|/)win/|^ios/")
old = [l.split(" ", 1) for l in open(S + "/stack.txt").read().strip().split("\n")]
new = out("git", "log", "--reverse", "--format=%H", "157.0.8103.78..port157-carried").split()
fix = {}
for h in out("git", "log", "--reverse", "--format=%H", "port157-carried..HEAD").split():
    s = out("git", "log", "-1", "--format=%s", h).strip()
    if s.startswith("fixup! "):
        fix.setdefault(s[7:], set()).update(x for x in out("git", "show", "--name-only", "--format=", h).split("\n") if x)
PL = patch_logs(S + "/resolve.log")
tot = {"logged": 0, "android_win": 0, "present": 0, "unaccounted": 0}
for i, ((oh, subj), nh) in enumerate(zip(old, new), 1):
    of = [l for l in out("git", "show", "--name-only", "--format=", oh).split("\n") if l and not l.startswith(SUBS)]
    nf = set(x for x in out("git", "show", "--name-only", "--format=", nh).split("\n") if x) | fix.get(subj, set())
    for f in of:
        if f in nf: continue
        if f in PL.get(i, ""): tot["logged"] += 1; continue
        if ANDROID.search(f): tot["android_win"] += 1; continue
        d = sh("git", "diff", "--binary", oh + "^", oh, "--", f).stdout
        if d and sh("git", "apply", "--check", "-R", "-", inp=d).returncode == 0:
            tot["present"] += 1; print(f"PRESENT {i} {f}"); continue
        tot["unaccounted"] += 1; print(f"UNACCOUNTED {i} {subj[:50]} :: {f}")
print(tot)
