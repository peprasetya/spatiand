#!/usr/bin/env python3
"""A HEIC written by hand around HEVC that x265 (through ffmpeg) coded: there is no HEIF encoder on
this machine (libheif has only its AV1 plugins; ffmpeg has no HEIF muxer), so the container --
ftyp, meta (hdlr, pitm, iinf, iref, iprp with hvcC, ispe, colr, irot, pixi, ipma; a grid's
dimg and its ImageGrid), iloc, mdat -- is written here to ISO/IEC 23008-12, as an iPhone's is.

usage: heif.py out.heic --tile T.hevc [--tile ...] [--grid cols rows width height]
               [--width W --height H] [--bits 8|10] [--icc file] [--nclx P T M FULL]
               [--irot quarters] [--exif file]
Each tile is an Annex B HEVC stream of one intra picture; their parameter sets must agree.
"""
import argparse
import struct


def box(kind, body):
    return struct.pack(">I", 8 + len(body)) + kind + body


def full(kind, version, flags, body):
    return box(kind, struct.pack(">I", (version << 24) | flags) + body)


def nals(stream):
    """The NAL units of an Annex B stream."""
    out, i, n = [], 0, len(stream)
    starts = []
    while i < n - 3:
        if stream[i:i + 3] == b"\0\0\1":
            starts.append(i + 3)
            i += 3
        else:
            i += 1
    for k, s in enumerate(starts):
        e = starts[k + 1] - 3 if k + 1 < len(starts) else n
        unit = stream[s:e]
        while unit.endswith(b"\0"):
            unit = unit[:-1]
        out.append(unit)
    return out


def rbsp(unit):
    out, zeros = bytearray(), 0
    for b in unit:
        if zeros >= 2 and b == 3:
            zeros = 0
            continue
        out.append(b)
        zeros = zeros + 1 if b == 0 else 0
    return bytes(out)


def hvcc(vps, sps, pps, bits):
    s = rbsp(sps)[2:]  # past the NAL header
    ptl = s[1:13]      # profile_tier_level's general part, byte aligned after the first byte
    body = bytes([1]) + ptl[0:1] + ptl[1:5] + ptl[5:11] + ptl[11:12]
    body += struct.pack(">H", 0xF000)          # min_spatial_segmentation_idc 0
    body += bytes([0xFC])                       # parallelismType 0
    body += bytes([0xFC | 1])                   # chroma_format_idc 1 (4:2:0)
    body += bytes([0xF8 | (bits - 8)]) * 2      # bit depths
    body += struct.pack(">H", 0)                # avgFrameRate
    body += bytes([(0 << 6) | (1 << 3) | (1 << 2) | 3])  # 1 temporal layer, nested, 4-byte lengths
    arrays = [(32, vps), (33, sps), (34, pps)]
    body += bytes([len(arrays)])
    for kind, unit in arrays:
        body += bytes([0x80 | kind]) + struct.pack(">H", 1) + struct.pack(">H", len(unit)) + unit
    return box(b"hvcC", body)


def coded(stream):
    """An item's data, its slices length-prefixed; and its parameter sets."""
    units = nals(stream)
    params = {}
    data = b""
    for unit in units:
        kind = (unit[0] >> 1) & 0x3F
        if kind in (32, 33, 34):
            params.setdefault(kind, unit)
        elif kind < 32:
            data += struct.pack(">I", len(unit)) + unit
    return data, params


def main():
    a = argparse.ArgumentParser()
    a.add_argument("out")
    a.add_argument("--tile", action="append", required=True)
    a.add_argument("--grid", nargs=4, type=int)
    a.add_argument("--width", type=int)
    a.add_argument("--height", type=int)
    a.add_argument("--bits", type=int, default=8)
    a.add_argument("--icc")
    a.add_argument("--nclx", nargs=4, type=int)
    a.add_argument("--irot", type=int, default=0)
    a.add_argument("--exif")
    o = a.parse_args()

    tiles = [coded(open(t, "rb").read()) for t in o.tile]
    params = tiles[0][1]
    grid = o.grid
    # Items: the tiles 1..n; the grid (if any) n+1, the primary; Exif after.
    n = len(tiles)
    primary = n + 1 if grid else 1
    exif_id = primary + 1 if o.exif else None
    exif = b""
    if o.exif:
        raw = open(o.exif, "rb").read()
        if not raw.startswith(b"Exif\0\0"):
            raw = b"Exif\0\0" + raw
        exif = struct.pack(">I", 6) + raw  # the TIFF header's offset in what follows
    grid_data = b""
    if grid:
        cols, rows, w, h = grid
        large = w > 65535 or h > 65535
        grid_data = bytes([0, 1 if large else 0, rows - 1, cols - 1]) + (struct.pack(">II", w, h) if large else struct.pack(">HH", w, h))

    # Properties: 1 hvcC, 2 ispe of a tile (or of the image), 3 pixi, [4 ispe of the grid], then colr, irot.
    props = [hvcc(params[32], params[33], params[34], o.bits)]
    tile_w, tile_h = o.width, o.height
    props.append(full(b"ispe", 0, 0, struct.pack(">II", tile_w, tile_h)))
    props.append(full(b"pixi", 0, 0, bytes([3, o.bits, o.bits, o.bits])))
    grid_ispe = None
    if grid:
        props.append(full(b"ispe", 0, 0, struct.pack(">II", grid[2], grid[3])))
        grid_ispe = len(props)
    colour = []
    if o.icc:
        props.append(box(b"colr", b"prof" + open(o.icc, "rb").read()))
        colour.append(len(props))
    if o.nclx:
        p, t, m, f = o.nclx
        props.append(box(b"colr", b"nclx" + struct.pack(">HHHB", p, t, m, 0x80 if f else 0)))
        colour.append(len(props))
    turn = None
    if o.irot:
        props.append(box(b"irot", bytes([o.irot & 3])))
        turn = len(props)

    # Which item has which: (index, essential).
    assoc = {}
    for i in range(1, n + 1):
        assoc[i] = [(1, True), (2, False), (3, False)] + ([] if grid else [(c, True) for c in colour] + ([(turn, True)] if turn else []))
    if grid:
        assoc[primary] = [(grid_ispe, False), (3, False)] + [(c, True) for c in colour] + ([(turn, True)] if turn else [])
    ipma = struct.pack(">I", len(assoc))
    for item, these in assoc.items():
        ipma += struct.pack(">HB", item, len(these)) + bytes([(0x80 if e else 0) | k for k, e in these])
    iprp = box(b"iprp", box(b"ipco", b"".join(props)) + full(b"ipma", 0, 0, ipma))

    infes = []
    for i in range(1, n + 1):
        hidden = 1 if grid else 0
        infes.append(full(b"infe", 2, hidden, struct.pack(">HH", i, 0) + b"hvc1" + b"\0"))
    if grid:
        infes.append(full(b"infe", 2, 0, struct.pack(">HH", primary, 0) + b"grid" + b"\0"))
    if exif_id:
        infes.append(full(b"infe", 2, 0, struct.pack(">HH", exif_id, 0) + b"Exif" + b"\0"))
    iinf = full(b"iinf", 0, 0, struct.pack(">H", len(infes)) + b"".join(infes))

    refs = b""
    if grid:
        refs += box(b"dimg", struct.pack(">HH", primary, n) + b"".join(struct.pack(">H", i) for i in range(1, n + 1)))
    if exif_id:
        refs += box(b"cdsc", struct.pack(">HHH", exif_id, 1, primary))
    iref = full(b"iref", 0, 0, refs) if refs else b""

    hdlr = full(b"hdlr", 0, 0, struct.pack(">I", 0) + b"pict" + b"\0" * 12 + b"\0")
    pitm = full(b"pitm", 0, 0, struct.pack(">H", primary))

    # The data, in mdat: each tile, then the grid's description, then the Exif.
    pieces = [(i + 1, tiles[i][0]) for i in range(n)]
    if grid:
        pieces.append((primary, grid_data))
    if exif_id:
        pieces.append((exif_id, exif))

    def iloc_box(at):
        body = bytes([0x44, 0x00]) + struct.pack(">H", len(pieces))
        offset = at
        for item, data in pieces:
            body += struct.pack(">HHHII", item, 0, 1, offset, len(data))
            offset += len(data)
        return full(b"iloc", 0, 0, body)

    ftyp = box(b"ftyp", b"heic" + struct.pack(">I", 0) + b"mif1heic")
    # Two passes: the offsets depend on the meta box's length, which does not depend on them.
    meta = full(b"meta", 0, 0, hdlr + pitm + iinf + iref + iprp + iloc_box(0))
    start = len(ftyp) + len(meta) + 8
    meta = full(b"meta", 0, 0, hdlr + pitm + iinf + iref + iprp + iloc_box(start))
    mdat = box(b"mdat", b"".join(d for _, d in pieces))
    open(o.out, "wb").write(ftyp + meta + mdat)


main()
