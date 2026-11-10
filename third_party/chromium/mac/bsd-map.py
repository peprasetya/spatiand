#!/usr/bin/env python3
"""Where Chromium's Linux UI takes its Linux path on a BSD kernel, the Mac's takes it too.

The FreeBSD port of Chromium (FreeBSD ports, www/chromium/files) builds the Linux UI on a BSD
kernel, macOS's family, and widens Chromium's Linux checks to BSD wherever that path is the
right one there: `BUILDFLAG(IS_LINUX)` becomes `BUILDFLAG(IS_LINUX) || BUILDFLAG(IS_BSD)`. This
reads those widenings and makes the same ones in the Mac's tree with BUILDFLAG(IS_MAC_LINUX_UI)
(build/build_config.h) -- a map of where, each one still the Mac tree's own line:

  * only a hunk whose removed and added line differ by the BSD check alone, and only where the
    removed line is found in the Mac tree as it is (else it is reported, not guessed at);
  * only in the UI's directories (DIRS): below the UI the system is macOS's own (IS_MAC), and
    what FreeBSD does there for itself (its allocator, crash handler, devices) is not the Mac's;
  * a check that already takes the Mac (IS_MAC or IS_APPLE in it, not negated) is left alone;
  * FreeBSD- and OpenBSD-only checks (IS_FREEBSD, IS_OPENBSD) are left out: they are those
    systems', not BSD's;
  * where an earlier branch of the same #if chain takes the Mac first, the widening does nothing
    yet: reported as "Mac first", for a person to decide.

  bsd-map.py <freebsd www/chromium/files> <chromium src> [--apply]

Without --apply it says what it would do. GN files are not touched: FreeBSD makes GN's is_linux
true on BSD, which the Mac cannot (is_mac must stay true for its toolchain); the Mac's BUILD.gn
changes are made by hand (is_mac_linux_ui).
"""

import os
import re
import sys

DIRS = (
    "chrome/browser/ui/",
    "ui/",
    "components/constrained_window/",
    "components/web_modal/",
    "components/eye_dropper/",
)
# Inside those, what is the system's, not the UI's: the certificates are NSS's on Linux and the
# Keychain's on the Mac. (content/browser is left out whole: its renderer_host holds process
# management beside the views; it is done by hand.)
EXCLUDE = (
    "chrome/browser/ui/webui/certificate_manager/",
)

BSD = re.compile(r"\s*(\|\||&&)\s*!?BUILDFLAG\(IS_BSD\)")
MAC_FIRST = re.compile(r"(?<!!)BUILDFLAG\((IS_MAC|IS_APPLE)\)")


def hunks(text):
    """(path, [(old block, new block, old start line)]): each hunk as the lines it expects and
    the lines it leaves, context included, so that it is placed as `patch` places it."""
    m = re.search(r"^\+\+\+ (\S+)", text, re.M)
    if not m:
        return None, []
    path = m.group(1)
    out = []
    for h in re.split(r"^(?=@@ )", text, flags=re.M)[1:]:
        head = re.match(r"@@ -(\d+)", h)
        old, new = [], []
        for line in h.splitlines()[1:]:
            if line.startswith("\\"):
                continue
            tag, body = line[:1], line[1:]
            if tag in (" ", ""):
                old.append(body)
                new.append(body)
            elif tag == "-":
                old.append(body)
            elif tag == "+":
                new.append(body)
        out.append(("\n".join(old), "\n".join(new), int(head.group(1)) if head else 0))
    return path, out


def pairs_of(old, new):
    """The changed lines of a hunk, one for one where the counts agree."""
    import difflib
    a, b = old.split("\n"), new.split("\n")
    out = []
    for tag, i1, i2, j1, j2 in difflib.SequenceMatcher(None, a, b, autojunk=False).get_opcodes():
        if tag == "replace":
            if i2 - i1 == j2 - j1:
                out.extend(zip(a[i1:i2], b[j1:j2]))
            else:
                out.append(("\n".join(a[i1:i2]), "\n".join(b[j1:j2])))
    return out


def chain_takes_mac_first(lines, at):
    """Whether an earlier #if/#elif of the chain `at` belongs to takes the Mac."""
    if not re.match(r"\s*#\s*elif", lines[at]):
        return False
    depth = 0
    for j in range(at - 1, -1, -1):
        s = lines[j].strip()
        if re.match(r"#\s*endif", s):
            depth += 1
        elif re.match(r"#\s*if", s):
            if depth == 0:
                return bool(MAC_FIRST.search(s))
            depth -= 1
        elif depth == 0 and re.match(r"#\s*elif", s) and MAC_FIRST.search(s):
            return True
    return False


def main():
    files, src = sys.argv[1], sys.argv[2]
    apply = "--apply" in sys.argv
    done = missing = mac_first = skipped = 0
    report = []
    for name in sorted(os.listdir(files)):
        if not name.startswith("patch-"):
            continue
        path, pairs = hunks(open(os.path.join(files, name), errors="replace").read())
        if not path or not path.startswith(DIRS) or path.startswith(EXCLUDE) or not path.endswith((".cc", ".h", ".mm")):
            continue
        target = os.path.join(src, path)
        if not os.path.exists(target):
            report.append(f"no file      {path}")
            continue
        text = open(target).read()
        changed = text
        for old_block, new_block, start in pairs:
            todo = []
            for old, new in pairs_of(old_block, new_block):
                if "IS_BSD" not in new or "IS_BSD" in old:
                    continue
                if re.search(r"IS_FREEBSD|IS_OPENBSD", new) or BSD.sub("", new) != old or MAC_FIRST.search(old):
                    skipped += 1
                    continue
                todo.append((old, new.replace("BUILDFLAG(IS_BSD)", "BUILDFLAG(IS_MAC_LINUX_UI)")))
            if not todo:
                continue
            # Where: the hunk's lines as they are, else the first changed line nearest the
            # line the hunk names (Chromium has moved on a little since the Mac's tree).
            at = -1
            if changed.count(old_block) == 1:
                at = changed.index(old_block)
            else:
                wanted = len("\n".join(changed.split("\n")[: max(start - 1, 0)]))
                spots = [m.start() for m in re.finditer(re.escape(todo[0][0]), changed)]
                if spots:
                    best = min(spots, key=lambda p: abs(p - wanted))
                    if abs(changed[:best].count("\n") - start) < 400:
                        at = best
            if at < 0:
                missing += len(todo)
                report.append(f"not found    {path}:{start}: {todo[0][0].strip()[:90]}")
                continue
            for old, ours in todo:
                i = changed.find(old, at)
                if i < 0:
                    missing += 1
                    report.append(f"not found    {path}:{start}: {old.strip()[:90]}")
                    continue
                line = changed[:i].count("\n")
                if chain_takes_mac_first(changed.split("\n"), line):
                    mac_first += 1
                    report.append(f"Mac first    {path}:{line + 1}: {old.strip()[:90]}")
                changed = changed[:i] + ours + changed[i + len(old):]
                done += 1
        if apply and changed != text:
            open(target, "w").write(changed)
    print("\n".join(report))
    print(f"{done} widened{' (applied)' if apply else ''}, {mac_first} of them behind a Mac branch, "
          f"{missing} not found in this tree, {skipped} left out (FreeBSD's own, or already the Mac's)")


if __name__ == "__main__":
    main()
