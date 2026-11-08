#!/usr/bin/env python3
"""
Record what a touchpad actually reports through a swipe, a press and a lift.

Written because the scroll filter has now been guessed at twice and been wrong twice. Every
threshold in `spatiand-input::scroll` is a claim about how the hardware behaves across those
three moments, and none of them had ever been checked against the hardware doing it.

Writes a CSV of raw reports. `tools/analyse-pad-trace.py` reads it back, decimates it to the
frame rate the compositor actually samples at, and says what the filter would have done.

Both pads are recorded whatever you touch, so one run covers either hand.

    capture-pad-trace.py [out.csv] [--passive SECONDS]

`--passive` records for that long without claiming the controller and without the prompts: for
while Spatiand's room has it, which already reads the pads in the mode recorded here, so the
wearer can just use the room (a thumb held on a button, a slow scroll) while this listens.
"""
import fcntl, glob, os, select, struct, sys, time

VID_PID = "0003:000028DE:00001205"
HEADER = bytes([0x01, 0x00, 0x09, 0x40])

ID_CLEAR_DIGITAL_MAPPINGS = 0x81
ID_SET_SETTINGS_VALUES = 0x87
ID_LOAD_DEFAULT_SETTINGS = 0x8E
REG_LPAD_MODE = 0x07
REG_RPAD_MODE = 0x08
REG_GYRO_MODE = 0x30

BIT_LPAD_CLICK = 17
BIT_RPAD_CLICK = 18
BIT_LPAD_TOUCH = 19
BIT_RPAD_TOUCH = 20


def hidiocsfeature(size):
    return (3 << 30) | (size << 16) | (0x48 << 8) | 0x06


def send_feature(fd, payload):
    buf = bytearray(65)
    buf[1 : 1 + len(payload)] = payload
    fcntl.ioctl(fd, hidiocsfeature(len(buf)), bytes(buf))


def find_node():
    # The vendor interface, 2, which carries the reports read here and takes the feature reports:
    # the controller shows three, and which is numbered first changes from boot to boot.
    for path in sorted(glob.glob("/sys/class/hidraw/hidraw*")):
        try:
            if VID_PID not in open(os.path.join(path, "device", "uevent")).read():
                continue
        except OSError:
            continue
        if os.path.realpath(os.path.join(path, "device")).split("/")[-2].endswith(".2"):
            return "/dev/" + os.path.basename(path)
    return None


def take(fd):
    send_feature(fd, bytes([ID_CLEAR_DIGITAL_MAPPINGS, 0]))
    send_feature(fd, bytes([ID_LOAD_DEFAULT_SETTINGS, 0]))
    payload = bytearray([ID_SET_SETTINGS_VALUES, 9])
    for reg, value in ((REG_LPAD_MODE, 0x07), (REG_RPAD_MODE, 0x07), (REG_GYRO_MODE, 0x18)):
        payload += struct.pack("<BH", reg, value)
    send_feature(fd, bytes(payload))


def main():
    args = sys.argv[1:]
    passive = None
    if "--passive" in args:
        i = args.index("--passive")
        passive = float(args[i + 1])
        del args[i : i + 2]
    out_path = args[0] if args else "/tmp/pad-trace.csv"
    node = find_node()
    if not node:
        sys.exit("no Valve controller found")
    if passive is not None:
        # Read-only: the room has claimed the controller already, and claiming it again would
        # reset what the room set up.
        fd = os.open(node, os.O_RDONLY | os.O_NONBLOCK)
        print(f"reading {node} for {passive:.0f} s, writing {out_path}")
    else:
        fd = os.open(node, os.O_RDWR | os.O_NONBLOCK)
        take(fd)
        time.sleep(0.3)

        print(f"reading {node}, writing {out_path}\n")
        print("Do these three things on the LEFT pad, pausing a beat between them:")
        print("  1. a slow steady swipe up, then lift off cleanly")
        print("  2. a quick flick, then lift off")
        print("  3. rest your thumb, press the pad down to click, release, then lift")
        print("\nRecording for 20 s starting now...")

    rows = []
    t0 = time.time()
    end = t0 + (passive if passive is not None else 20.0)
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.2)
        if not r:
            continue
        data = os.read(fd, 64)
        if len(data) < 64 or data[:4] != HEADER:
            continue
        seq = struct.unpack_from("<I", data, 4)[0]
        buttons = struct.unpack_from("<Q", data, 8)[0]
        lx, ly = struct.unpack_from("<hh", data, 16)
        rx, ry = struct.unpack_from("<hh", data, 20)
        lp = struct.unpack_from("<H", data, 56)[0]
        rp = struct.unpack_from("<H", data, 58)[0]
        rows.append(
            (
                round(time.time() - t0, 5),
                seq,
                lx, ly,
                int(bool(buttons & (1 << BIT_LPAD_TOUCH))),
                int(bool(buttons & (1 << BIT_LPAD_CLICK))),
                lp,
                rx, ry,
                int(bool(buttons & (1 << BIT_RPAD_TOUCH))),
                int(bool(buttons & (1 << BIT_RPAD_CLICK))),
                rp,
            )
        )
    os.close(fd)

    with open(out_path, "w") as f:
        f.write("t,seq,lx,ly,ltouch,lclick,lpressure,rx,ry,rtouch,rclick,rpressure\n")
        for row in rows:
            f.write(",".join(str(v) for v in row) + "\n")

    touched = sum(1 for r in rows if r[4] or r[9])
    clicked = sum(1 for r in rows if r[5] or r[10])
    print(f"\n{len(rows)} reports, {touched} with a thumb down, {clicked} with a click held")
    if touched < 100:
        print("WARNING: very little contact recorded — was a thumb on a pad?")
    if clicked == 0 and passive is None:
        print("WARNING: no click recorded — step 3 is the important one")
    print(f"wrote {out_path}")


if __name__ == "__main__":
    main()
