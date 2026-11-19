#!/usr/bin/env python3
"""The pictures SpatiLoupe's tests copy and save (save.rs), made on deepmagpie with what it has:
PIL for the pattern, lcms2 (ctypes) for a Display P3 profile, ffmpeg (x265, libaom, libjxl,
OpenJPEG) for the coding, and heif.py for the HEIF container x265's output has no muxer for
there (libheif has only its AV1 plugins; ffmpeg writes no HEIF).

usage: make.py [work dir]    (the pictures the tests use are copied here, beside this)
"""
import ctypes
import os
import shutil
import struct
import subprocess
import sys
import tempfile

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = sys.argv[1] if len(sys.argv) > 1 else tempfile.mkdtemp()
os.makedirs(OUT, exist_ok=True)
W, H = 480, 360


def p(name):
    return os.path.join(OUT, name)


def run(*args):
    subprocess.run(args, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)


# The pattern, as stored (landscape): four quarters of known colour, a grey ramp across the foot.
QUARTERS = [(200, 40, 40), (40, 180, 60), (40, 60, 200), (220, 220, 220)]


def colour(x, y):
    if y >= H - 40:
        v = round(x * 255 / (W - 1))
        return (v, v, v)
    q = (1 if x >= W // 2 else 0) + (2 if y >= (H - 40) // 2 else 0)
    return QUARTERS[q]


img = Image.new("RGB", (W, H))
img.putdata([colour(x, y) for y in range(H) for x in range(W)])
img.save(p("pattern.png"))

# A 16-bit ramp, for the deep ones: every value of 16 bits' worth along it, not 8's.
deep = Image.new("I;16", (W, H))
deep.putdata([(x * 65535 // (W - 1)) ^ (y & 0x7) for y in range(H) for x in range(W)])

# Display P3, made by lcms2: P3's primaries, D65, sRGB's curve.
lcms = ctypes.CDLL("liblcms2.so.2")


class XyY(ctypes.Structure):
    _fields_ = [("x", ctypes.c_double), ("y", ctypes.c_double), ("Y", ctypes.c_double)]


class Triple(ctypes.Structure):
    _fields_ = [("r", XyY), ("g", XyY), ("b", XyY)]


lcms.cmsBuildParametricToneCurve.restype = ctypes.c_void_p
lcms.cmsBuildParametricToneCurve.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.POINTER(ctypes.c_double)]
lcms.cmsCreateRGBProfile.restype = ctypes.c_void_p
lcms.cmsCreateRGBProfile.argtypes = [ctypes.POINTER(XyY), ctypes.POINTER(Triple), ctypes.c_void_p]
lcms.cmsMLUalloc.restype = ctypes.c_void_p
lcms.cmsMLUalloc.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
lcms.cmsMLUsetASCII.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p]
lcms.cmsWriteTag.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_void_p]
lcms.cmsSaveProfileToMem.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint32)]
params = (ctypes.c_double * 5)(2.4, 1 / 1.055, 0.055 / 1.055, 1 / 12.92, 0.04045)
curve = lcms.cmsBuildParametricToneCurve(None, 4, params)
curves = (ctypes.c_void_p * 3)(curve, curve, curve)
white = XyY(0.3127, 0.3290, 1.0)
primaries = Triple(XyY(0.680, 0.320, 1.0), XyY(0.265, 0.690, 1.0), XyY(0.150, 0.060, 1.0))
prof = lcms.cmsCreateRGBProfile(ctypes.byref(white), ctypes.byref(primaries), curves)
mlu = lcms.cmsMLUalloc(None, 1)
lcms.cmsMLUsetASCII(mlu, b"en", b"US", b"Display P3")
lcms.cmsWriteTag(prof, 0x64657363, mlu)
size = ctypes.c_uint32()
lcms.cmsSaveProfileToMem(prof, None, ctypes.byref(size))
buf = ctypes.create_string_buffer(size.value)
lcms.cmsSaveProfileToMem(prof, buf, ctypes.byref(size))
icc = buf.raw[: size.value]
open(p("display-p3.icc"), "wb").write(icc)


# An iPhone's Exif: big-endian, turned a quarter (6), Apple, a time, the stored size, colour
# space uncalibrated (P3).
def exif(width, height):
    entries0 = []
    data = b""
    base = 8

    def ifd(entries, at, next_at=0):
        # entries: (tag, type, count, value bytes)
        out = struct.pack(">H", len(entries))
        extra = b""
        extra_at = at + 2 + 12 * len(entries) + 4
        for tag, kind, count, value in entries:
            if len(value) <= 4:
                out += struct.pack(">HHI", tag, kind, count) + value.ljust(4, b"\0")
            else:
                out += struct.pack(">HHII", tag, kind, count, extra_at + len(extra))
                extra += value + (b"\0" if len(value) % 2 else b"")
        return out + struct.pack(">I", next_at) + extra

    make, model = b"Apple\0", b"iPhone 15 Pro\0"
    # IFD0 at 8 with five entries: its length is fixed, so the Exif IFD's place is known.
    ifd0_len = len(ifd([(0x010F, 2, len(make), make), (0x0110, 2, len(model), model), (0x0112, 3, 1, struct.pack(">H", 6)), (0x0131, 2, 5, b"17.1\0"), (0x8769, 4, 1, b"\0\0\0\0")], base))
    exif_at = base + ifd0_len
    first = ifd([(0x010F, 2, len(make), make), (0x0110, 2, len(model), model), (0x0112, 3, 1, struct.pack(">H", 6)), (0x0131, 2, 5, b"17.1\0"), (0x8769, 4, 1, struct.pack(">I", exif_at))], base)
    when = b"2026:11:19 10:00:00\0"
    inner = ifd([(0x9003, 2, len(when), when), (0xA001, 3, 1, struct.pack(">H", 0xFFFF)), (0xA002, 4, 1, struct.pack(">I", width)), (0xA003, 4, 1, struct.pack(">I", height))], exif_at)
    return b"MM\0*" + struct.pack(">I", 8) + first + inner


open(p("iphone.exif"), "wb").write(exif(W, H))


def hevc(src, out, bits=8, full=True, matrix="bt601", extra=()):
    fmt = "yuv420p10le" if bits == 10 else "yuv420p"
    rng = "pc" if full else "tv"
    run("ffmpeg", "-y", "-loglevel", "error", "-i", src, "-vf", f"scale=out_color_matrix={matrix}:out_range={rng},format={fmt}",
        "-color_range", rng, "-colorspace", "smpte170m" if matrix == "bt601" else matrix, *extra,
        "-c:v", "libx265", "-x265-params", "keyint=1:log-level=error:crf=6", "-frames:v", "1", "-f", "hevc", out)


# 1. An iPhone-like HEIC: a 2 x 2 grid of 256 x 192 tiles (the whole 480 x 360, cropped from
# 512 x 384), turned a quarter by irot (shown 360 wide and 480 tall), Display P3 by ICC and by
# nclx (P3, sRGB's curve, BT.601, full range), the Exif above.
padded = Image.new("RGB", (512, 384))
padded.paste(img, (0, 0))
padded.paste(img.crop((W - 1, 0, W, H)).resize((512 - W, H)), (W, 0))
padded.paste(padded.crop((0, H - 1, 512, H)).resize((512, 384 - H)), (0, H))
tiles = []
for row in range(2):
    for col in range(2):
        t = p(f"tile{row}{col}.png")
        padded.crop((col * 256, row * 192, col * 256 + 256, row * 192 + 192)).save(t)
        hevc(t, t.replace(".png", ".hevc"), extra=("-color_primaries", "smpte432", "-color_trc", "iec61966-2-1"))
        tiles.append(t.replace(".png", ".hevc"))
args = ["python3", "-I", os.path.join(HERE, "heif.py"), p("iphone.heic")]
for t in tiles:
    args += ["--tile", t]
args += ["--grid", "2", "2", str(W), str(H), "--width", "256", "--height", "192", "--icc", p("display-p3.icc"), "--nclx", "12", "13", "6", "1", "--irot", "3", "--exif", p("iphone.exif")]
run(*args)

# 2. A plain HEIC, one image, not turned, BT.709 limited range (as most cameras but Apple's).
hevc(p("pattern.png"), p("plain.hevc"), full=False, matrix="bt709", extra=("-color_primaries", "bt709", "-color_trc", "bt709"))
run("python3", "-I", os.path.join(HERE, "heif.py"), p("plain.heic"), "--tile", p("plain.hevc"), "--width", str(W), "--height", str(H), "--nclx", "1", "1", "1", "0")

# 3. A ten-bit HEIC of the 16-bit ramp (Main 10), BT.709 full range.
ramp16 = p("ramp16.png")
# PIL writes 16-bit grey; ffmpeg makes it RGB48 on the way.
deep.save(ramp16)
hevc(ramp16, p("ramp10.hevc"), bits=10, full=True, matrix="bt709", extra=("-color_primaries", "bt709", "-color_trc", "bt709"))
run("python3", "-I", os.path.join(HERE, "heif.py"), p("ramp10.heic"), "--tile", p("ramp10.hevc"), "--width", str(W), "--height", str(H), "--bits", "10", "--nclx", "1", "1", "1", "1")

# 4. AVIF, eight and ten bits (libaom, ffmpeg's AVIF muxer).
for src, out, fmt in ((p("pattern.png"), p("pattern.avif"), "yuv420p"), (ramp16, p("ramp10.avif"), "yuv420p10le")):
    run("ffmpeg", "-y", "-loglevel", "error", "-i", src, "-vf", f"scale=out_color_matrix=bt709:out_range=pc,format={fmt}", "-color_range", "pc",
        "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-c:v", "libaom-av1", "-still-picture", "1", "-crf", "4", "-cpu-used", "8", out)

# 4b. An HDR10 AVIF: ten bits, BT.2020, PQ (the frame's own parameters, which ffmpeg's encoder
# takes over its options).
run("ffmpeg", "-y", "-loglevel", "error", "-i", ramp16, "-vf", "scale=out_color_matrix=bt2020:out_range=pc,format=yuv420p10le,setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc:range=pc",
    "-c:v", "libaom-av1", "-still-picture", "1", "-crf", "4", "-cpu-used", "8", p("hdr10.avif"))

# 5. JPEG XL, eight and sixteen bits (libjxl); JPEG 2000, eight and sixteen (OpenJPEG).
run("ffmpeg", "-y", "-loglevel", "error", "-i", p("pattern.png"), "-c:v", "libjxl", "-distance", "0.5", p("pattern.jxl"))
run("ffmpeg", "-y", "-loglevel", "error", "-i", ramp16, "-pix_fmt", "rgb48le", "-c:v", "libjxl", "-distance", "0", p("ramp16.jxl"))
run("ffmpeg", "-y", "-loglevel", "error", "-i", p("pattern.png"), "-c:v", "libopenjpeg", p("pattern.jp2"))
run("ffmpeg", "-y", "-loglevel", "error", "-i", ramp16, "-pix_fmt", "rgb48le", "-c:v", "libopenjpeg", p("ramp16.jp2"))
for f in ("iphone.heic", "plain.heic", "pattern.avif", "pattern.jxl", "pattern.jp2", "ramp10.heic", "ramp10.avif", "hdr10.avif", "ramp16.jxl"):
    shutil.copy(p(f), os.path.join(HERE, f))
    print(f, os.path.getsize(p(f)))
