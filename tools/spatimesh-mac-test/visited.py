#!/usr/bin/env python3
"""Seeds the rig's shared folder with visits "another machine" made, for visited.html:

    visited.py seed            before sk.sh (a fresh /tmp/smtest/profile)
    visited.py read shot.png   says which of the page's three blocks are red (visited)

?a has no page with it (as every visit shared before 2026-11-12), ?b was followed on a page of
127.0.0.1. So at localhost a is visited and b is not; at 127.0.0.1 b is and a is not; c never.
"""
import json, os, struct, sys, time, zlib

if sys.argv[1] == "seed":
    d = "/tmp/smtest/apps/apps/spatimesh/profiles/default/history"
    os.makedirs(d, exist_ok=True)
    now = time.time() * 1000
    visits = [
        {"url": "http://localhost:18765/done.html?a", "title": "a", "at": now - 60000},
        {"url": "http://localhost:18765/done.html?b", "title": "b", "at": now - 50000,
         "top": "http://127.0.0.1:18765/visited.html", "frame": "http://127.0.0.1:18765/visited.html"},
    ]
    name = "othermachine0001-" + time.strftime("%Y-%m", time.gmtime())
    json.dump(visits, open(f"{d}/{name}.json", "w"))
    print("seeded", name)
else:
    # A PNG's pixels with nothing but zlib: 8-bit RGB or RGBA, any filter.
    b = open(sys.argv[2], "rb").read()
    at, chunks, head = 8, b"", None
    while at < len(b):
        n, kind = struct.unpack(">I4s", b[at:at + 8])
        if kind == b"IHDR": head = struct.unpack(">IIBB", b[at + 8:at + 18])
        if kind == b"IDAT": chunks += b[at + 8:at + 8 + n]
        at += 12 + n
    w, h, depth, colour = head
    bpp = 4 if colour == 6 else 3
    raw = zlib.decompress(chunks)
    rows, prev = [], bytearray(w * bpp)
    for y in range(h):
        f = raw[y * (w * bpp + 1)]
        line = bytearray(raw[y * (w * bpp + 1) + 1:(y + 1) * (w * bpp + 1)])
        for i in range(len(line)):
            a = line[i - bpp] if i >= bpp else 0
            up = prev[i]
            c = prev[i - bpp] if i >= bpp else 0
            if f == 1: line[i] = (line[i] + a) & 255
            elif f == 2: line[i] = (line[i] + up) & 255
            elif f == 3: line[i] = (line[i] + (a + up) // 2) & 255
            elif f == 4:
                p = a + up - c
                pa, pb, pc = abs(p - a), abs(p - up), abs(p - c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else up if pb <= pc else c)) & 255
        rows.append(line); prev = line
    scale = w / 1280
    out = []
    for i, name in enumerate("abc"):
        x, y = int(280 * scale), int((20 + i * 80 + 50) * scale)
        r, g, bl = rows[min(y, h - 1)][x * bpp:x * bpp + 3]
        out.append(f"{name}: {'visited' if r > 150 and bl < 100 else 'not visited' if bl > 150 and r < 100 else f'? {r},{g},{bl}'}")
    print(", ".join(out))
