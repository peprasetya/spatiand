#!/usr/bin/env python3
"""The crashed thread of a Crashpad minidump (macOS, arm64), walked by its frame pointers and
symbolised with atos -- for a SpatiMesh process that Crashpad caught (a renderer), where macOS
writes no report of its own.

    minidump.py <file.dmp>        on littleotter (atos is macOS's)
"""

import struct
import subprocess
import sys

import os
DSYM = os.path.expanduser("~/Developer/spatiand-chromium-mac/chromium/src/out/mac-ozone/SpatiMesh Framework.dSYM/Contents/Resources/DWARF/SpatiMesh Framework")
STREAM_THREADS, STREAM_MODULES, STREAM_EXCEPTION, STREAM_MEMORY = 3, 4, 6, 5


def u32(b, o):
    return struct.unpack_from("<I", b, o)[0]


def u64(b, o):
    return struct.unpack_from("<Q", b, o)[0]


def main(path):
    d = open(path, "rb").read()
    assert d[:4] == b"MDMP", "not a minidump"
    count, rva = u32(d, 8), u32(d, 12)
    streams = {}
    for i in range(count):
        t, size, at = struct.unpack_from("<III", d, rva + 12 * i)
        streams.setdefault(t, (size, at))

    def mdstring(at):
        n = u32(d, at)
        return d[at + 4 : at + 4 + n].decode("utf-16-le", "replace")

    # Modules: base, size, name
    modules = []
    size, at = streams[STREAM_MODULES]
    n = u32(d, at)
    for i in range(n):
        m = at + 4 + 108 * i
        base, msize = u64(d, m), u32(d, m + 8)
        modules.append((base, msize, mdstring(u32(d, m + 20))))

    def module_of(addr):
        for base, msize, name in modules:
            if base <= addr < base + msize:
                return base, name
        return None, None

    # The exception: which thread, what code, and its context
    size, at = streams[STREAM_EXCEPTION]
    tid = u32(d, at)
    code, flags = u32(d, at + 8), u32(d, at + 12)
    ex_addr = u64(d, at + 24)
    ctx_size, ctx_at = u32(d, at + 8 + 152), u32(d, at + 8 + 156)
    print(f"thread {tid:#x}: exception {code:#x} flags {flags:#x} at {ex_addr:#x}")

    # arm64 context (crashpad's MinidumpContextARM64): context_flags u32, cpsr u32, x0..x28,
    # fp (x29), lr (x30), sp, pc
    c = ctx_at
    regs = [u64(d, c + 8 + 8 * i) for i in range(29)]
    fp, lr, sp, pc = (u64(d, c + 8 + 8 * 29), u64(d, c + 8 + 8 * 30), u64(d, c + 8 + 8 * 31), u64(d, c + 8 + 8 * 32))

    # Memory the dump holds: from the thread list's stacks and the memory list
    regions = []
    size, at = streams[STREAM_THREADS]
    for i in range(u32(d, at)):
        t = at + 4 + 48 * i
        start, msize, mat = u64(d, t + 24), u32(d, t + 32), u32(d, t + 36)
        regions.append((start, msize, mat))
    if STREAM_MEMORY in streams:
        size, at = streams[STREAM_MEMORY]
        for i in range(u32(d, at)):
            r = at + 4 + 16 * i
            regions.append((u64(d, r), u32(d, r + 8), u32(d, r + 12)))

    def read64(addr):
        for start, msize, mat in regions:
            if start <= addr and addr + 8 <= start + msize:
                return u64(d, mat + addr - start)
        return None

    frames = [pc, lr]
    while fp and len(frames) < 40:
        ret = read64(fp + 8)
        nxt = read64(fp)
        if ret is None or nxt is None or nxt <= fp:
            break
        frames.append(ret)
        fp = nxt

    by_module = {}
    for i, a in enumerate(frames):
        base, name = module_of(a & 0x0000FFFFFFFFFFFF)
        by_module.setdefault((base, name), []).append((i, a & 0x0000FFFFFFFFFFFF))
    names = {}
    for (base, name), items in by_module.items():
        if not name:
            for i, a in items:
                names[i] = f"{a:#x} ?"
            continue
        try:
            out = subprocess.run(["atos", "-o", (DSYM if "SpatiMesh Framework" in name else name), "-l", hex(base)] + [hex(a) for _, a in items],
                                 capture_output=True, text=True, timeout=60).stdout.splitlines()
        except Exception as e:
            out = [str(e)] * len(items)
        for (i, a), sym in zip(items, out):
            names[i] = f"{name.rsplit('/', 1)[-1]} | {sym}"
    for i in range(len(frames)):
        print(f"{i:2} {names.get(i, '')[:200]}")


if __name__ == "__main__":
    main(sys.argv[1])
