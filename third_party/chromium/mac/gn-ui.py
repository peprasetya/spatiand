#!/usr/bin/env python3
"""The UI's build files take the Linux UI's sources where is_mac_linux_ui, the Cocoa UI's not.

The FreeBSD port makes GN's is_linux true on BSD, so its build files pick Linux's sources with
no change. The Mac cannot: is_mac drives its toolchain, SDK and bundles. So in the UI's build
files (ui/views, chrome/browser/ui, apps/ui) each condition says it:

    is_linux  ->  (is_linux || is_mac_linux_ui)
    is_mac    ->  (is_mac && !is_mac_linux_ui)

Line by line, and safe to run again:

  * a condition already rewritten is left as it is;
  * an assert() keeps plain is_mac: it says where a target may be built, which is not a choice
    of UI (other targets depend on it there);
  * a line under a `# Spatiand:` comment was decided by hand, and is left alone;
  * the Linux sandbox's page stays Linux's: below the UI the system is macOS's own;
  * WebUI pages (chrome/browser/ui/webui) keep macOS's: they are web content.

    gn-ui.py <dir>...      (in the Chromium tree's src)
"""

import os
import re
import sys

LINUX = re.compile(r"(?<![\w.])is_linux(?!\w)(?! \|\| is_mac_linux_ui)")
MAC = re.compile(r"(?<![\w.])is_mac(?!\w)(?! && !is_mac_linux_ui)")
SKIP_FILES = ("chrome/browser/ui/webui/sandbox/BUILD.gn",)
# WebUI pages are web content and keep macOS's, as their TypeScript does (preprocess_if_expr.py).
SKIP_DIRS = ("chrome/browser/ui/webui",)


def rewrite(lines):
    out = []
    in_assert = False
    for i, line in enumerate(lines):
        before = lines[i - 1] if i else ""
        after = lines[i + 1] if i + 1 < len(lines) else ""
        stripped = line.lstrip()
        if stripped.startswith("assert("):
            in_assert = True
        hand = "# Spatiand:" in before or "sandbox" in after or "sandbox" in line
        if not in_assert and not hand and not stripped.startswith("#"):
            line = LINUX.sub("(is_linux || is_mac_linux_ui)", line)
            line = MAC.sub("(is_mac && !is_mac_linux_ui)", line)
        if in_assert and ")" in line and line.count(")") >= line.count("("):
            in_assert = False
        out.append(line)
    return out


def main():
    files = changed = 0
    for root in sys.argv[1:]:
        for d, _, names in os.walk(root):
            if d.startswith(SKIP_DIRS):
                continue
            for n in names:
                if n != "BUILD.gn" and not n.endswith(".gni"):
                    continue
                p = os.path.join(d, n)
                if p.endswith(SKIP_FILES):
                    continue
                s = open(p).read()
                t = "\n".join(rewrite(s.split("\n")))
                files += 1
                if t != s:
                    open(p, "w").write(t)
                    changed += 1
    print(f"{changed} of {files} build files changed")


if __name__ == "__main__":
    main()
