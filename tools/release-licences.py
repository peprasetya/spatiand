#!/usr/bin/env python3
"""What a release's programs carry of others, and under which licences (tools/make-release.sh).

    release-licences.py rust <cargo metadata JSON> <package>... --list L --texts T
    release-licences.py go <program> [--modcache DIR] --list L --texts T

`rust`: every crate compiled into the named packages' programs -- their normal dependencies, on
the platform the metadata was filtered for, less the workspace's own -- with the licence each
declares, and the licence files each carries, alike ones written once with the crates they are
for. `go`: every module a Go program was built from, read from the build information Go writes
into it, so no Go is needed; with the module cache it was built from, each one's licence files
too, and the licence named from them.

L gets one line a crate or module ("name version licence"), T the texts. Writes nothing else.
"""

import json
import re
import sys
from pathlib import Path

LICENCE_FILE = re.compile(r"^(licen[cs]e|copying|notice|unlicense|copyright|patents)([-._ ].*)?$", re.IGNORECASE)


def licence_files(folder: Path):
    found = []
    for place in (folder, folder / "LICENSES", folder / "licenses"):
        if place.is_dir():
            found += sorted(p for p in place.iterdir() if p.is_file() and LICENCE_FILE.match(p.name))
    return found


def named(text: str, file_name: str = "") -> str:
    """The licence a licence file is, as near as its words say."""
    t = " ".join(text.split())
    lower = file_name.lower()
    if lower.startswith("notice"):
        return "NOTICE"
    if lower.startswith("patents") or "Additional IP Rights Grant (Patents)" in t:
        return "patent grant"
    if "Apache License" in t and "Version 2.0" in t:
        return "Apache-2.0"
    if "Mozilla Public License" in t and "2.0" in t:
        return "MPL-2.0"
    version = "-3.0" if "Version 3" in t else "-2.1" if "Version 2.1" in t else "-2.0" if "Version 2" in t else ""
    if "GNU LESSER GENERAL PUBLIC LICENSE" in t.upper():
        return "LGPL" + version
    if "GNU GENERAL PUBLIC LICENSE" in t.upper():
        return "GPL" + version
    if "CC0" in t:
        return "CC0-1.0"
    if "Permission is hereby granted, free of charge" in t:
        return "MIT"
    if "Redistribution and use in source and binary forms" in t:
        return "BSD-3-Clause" if ("Neither the name" in t or "endorse or promote" in t) else "BSD-2-Clause"
    if "Permission to use, copy, modify, and/or distribute" in t or "Permission to use, copy, modify, and distribute" in t:
        return "ISC"
    if "free and unencumbered software released into the public domain" in t:
        return "Unlicense"
    if "Creative Commons" in t:
        return "CC"
    return "see its text"


def write(list_path, texts_path, rows, texts, heading):
    """rows: (name, version, licence); texts: {text: [who]}."""
    with open(list_path, "w") as out:
        for name, version, licence in sorted(rows):
            out.write(f"{name} {version} {licence}\n")
    with open(texts_path, "w") as out:
        out.write(heading)
        for text, who in sorted(texts.items(), key=lambda kv: sorted(kv[1])[0]):
            out.write("\n" + "=" * 78 + "\n" + "For: " + ", ".join(sorted(set(who))) + "\n" + "=" * 78 + "\n\n")
            out.write(text.rstrip() + "\n")


def rust(metadata_path, packages, list_path, texts_path):
    metadata = json.load(open(metadata_path))
    by_id = {p["id"]: p for p in metadata["packages"]}
    members = set(metadata["workspace_members"])
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    wanted = [p["id"] for p in metadata["packages"] if p["id"] in members and p["name"] in packages]
    missing = set(packages) - {by_id[i]["name"] for i in wanted}
    if missing:
        sys.exit(f"release-licences: not in the workspace: {', '.join(sorted(missing))}")
    seen, todo = set(), list(wanted)
    while todo:
        node = nodes[todo.pop()]
        for dep in node["deps"]:
            if any(kind["kind"] is None for kind in dep["dep_kinds"]) and dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                todo.append(dep["pkg"])
    rows, texts = [], {}
    for pid in seen - members:
        p = by_id[pid]
        folder = Path(p["manifest_path"]).parent
        files = licence_files(folder)
        if p.get("license_file") and (folder / p["license_file"]).is_file():
            files.append(folder / p["license_file"])
        licence = p.get("license") or (named(files[0].read_text(errors="replace"), files[0].name) if files else "unstated")
        rows.append((p["name"], p["version"], licence))
        who = f'{p["name"]} {p["version"]}'
        for f in dict.fromkeys(files):
            texts.setdefault(f.read_text(errors="replace"), []).append(who)
        if not files:
            texts.setdefault(f"(No licence file is in this crate's package; it declares: {licence}.)", []).append(who)
    write(list_path, texts_path, rows, texts,
          "The licences of the Rust crates compiled into Spatiand's programs, as each crate's package\n"
          "carries them. Each text is given once, with the crates it is for.\n")
    return rows


def go_escape(path):
    # The module cache spells a capital as "!" and the letter in lower case.
    return re.sub(r"[A-Z]", lambda m: "!" + m.group(0).lower(), path)


def go(program, modcache, list_path, texts_path):
    data = Path(program).read_bytes()
    # The build information: "dep\t<path>\t<version>[\t<sum>]", each replaced by a "=>" line after it.
    deps = []
    for line in re.findall(rb"(?m)^(dep|=>)\t([^\t\n]+)\t([^\t\n]+)", data):
        kind, path, version = (x.decode() for x in line)
        if kind == "=>" and deps:
            deps[-1] = (path, version)
        elif kind == "dep":
            deps.append((path, version))
    deps = sorted(set(deps))
    if not deps:
        sys.exit(f"release-licences: no Go build information in {program}")
    rows, texts = [], {}
    for path, version in deps:
        licence = "see the module's source"
        folder = Path(modcache) / f"{go_escape(path)}@{version}" if modcache else None
        if folder and folder.is_dir():
            files = licence_files(folder)
            if files:
                licence = " and ".join(dict.fromkeys(named(f.read_text(errors="replace"), f.name) for f in files))
            for f in files:
                texts.setdefault(f.read_text(errors="replace"), []).append(f"{path} {version}")
        rows.append((path, version, licence))
    write(list_path, texts_path, rows, texts,
          "The licences of the Go modules linked into Spatiand's storage service, as each module's\n"
          "source carries them. Each text is given once, with the modules it is for.\n")
    return rows


def main(args):
    def option(name):
        if name in args:
            i = args.index(name)
            value = args[i + 1]
            del args[i:i + 2]
            return value
        return None

    list_path, texts_path, modcache = option("--list"), option("--texts"), option("--modcache")
    if not args or not list_path or not texts_path:
        sys.exit(__doc__)
    kind, rest = args[0], args[1:]
    if kind == "rust" and len(rest) >= 2:
        rows = rust(rest[0], set(rest[1:]), list_path, texts_path)
    elif kind == "go" and len(rest) == 1:
        rows = go(rest[0], modcache, list_path, texts_path)
    else:
        sys.exit(__doc__)
    copyleft = [f"{n} {v} ({l})" for n, v, l in rows if "GPL" in l]
    if copyleft:
        print("release-licences: GPL in what is linked, to be looked at: " + "; ".join(copyleft), file=sys.stderr)
    print(f"release-licences: {len(rows)} {kind} components")


if __name__ == "__main__":
    main(sys.argv[1:])
