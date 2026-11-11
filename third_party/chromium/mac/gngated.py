#!/usr/bin/env python3
"""GN twin of gated.py: for each line Spatiand added to a .gn/.gni file, the enclosing if() conditions
that mention an operating system. Also: Spatiand's added C++ lines that use BUILDFLAG(IS_LINUX) as a
value (not in a preprocessor line)."""
import re, subprocess, sys
base = sys.argv[1]
files = subprocess.run(["git","diff","--name-only",base],capture_output=True,text=True).stdout.split("\n")
ADD = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")
def added(f):
    d = subprocess.run(["git","diff","-U0",base,"--",f],capture_output=True,text=True,errors="replace").stdout
    out=[]
    for l in d.split("\n"):
        m=ADD.match(l)
        if m: out.extend(range(int(m.group(1)), int(m.group(1))+int(m.group(2) or "1")))
    return out
for f in files:
    if not f: continue
    if re.search(r"\.(gn|gni)$", f):
        try: src=open(f,errors="replace").read().split("\n")
        except OSError: continue
        stack=[]; cond={}
        pending=None
        for i,line in enumerate(src):
            s=re.sub(r"#.*$","",line)
            m=re.search(r"\b(if|else if)\s*\((.*)\)\s*\{\s*$", s)
            cond[i+1]=[c for c in stack if c]
            opens=s.count("{"); closes=s.count("}")
            # closes first (for "} else {")
            for _ in range(closes):
                if stack: last=stack.pop()
            if re.search(r"\}\s*else\s*\{", s):
                stack.append("else-of("+str(last)+")" if last else None); opens-=1
            elif m:
                c=m.group(2)
                stack.append(c if re.search(r"is_(linux|mac|chromeos|android|win|posix|apple|ios|fuchsia)|use_(ozone|aura|dbus|glib|udev)|toolkit_views",c) else None); opens-=1
            for _ in range(max(opens,0)): stack.append(None)
        groups={}
        for n in added(f):
            c=cond.get(n)
            if c and src[n-1].strip() and not src[n-1].strip().startswith("#"):
                groups.setdefault(" && ".join(c),[]).append(n)
        for k,v in groups.items():
            print(f"\n{f} [{k}] lines {v[0]}-{v[-1]} ({len(v)})")
            for n in v[:5]: print(f"    {n}: {src[n-1].strip()[:140]}")
    elif re.search(r"\.(cc|h|mm)$", f):
        try: src=open(f,errors="replace").read().split("\n")
        except OSError: continue
        for n in added(f):
            l=src[n-1]
            if "BUILDFLAG(IS_LINUX)" in l and not l.strip().startswith("#") and "IS_MAC_LINUX_UI" not in l:
                print(f"VALUE {f}:{n}: {l.strip()[:150]}")
