# The UI's build files: the Linux UI's sources where is_mac_linux_ui, the Cocoa UI's not.
import os, re, sys
roots = sys.argv[1:]
files = changed = 0
for root in roots:
    for d, _, names in os.walk(root):
        for n in names:
            if n != "BUILD.gn" and not n.endswith(".gni"):
                continue
            p = os.path.join(d, n)
            s = open(p).read()
            t = re.sub(r"(?<![\w.])is_linux(?!\w)", "(is_linux || is_mac_linux_ui)", s)
            t = re.sub(r"(?<![\w.])is_mac(?!\w)", "(is_mac && !is_mac_linux_ui)", t)
            files += 1
            if t != s:
                open(p, "w").write(t)
                changed += 1
print(f"{changed} of {files} build files changed")
