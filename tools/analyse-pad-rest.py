#!/usr/bin/env python3
"""
How much a thumb wanders while it means to be still, and what `spatiand-input::steady` leaves of it.

Reads a CSV from `tools/capture-pad-trace.py` (best `--passive`, recorded while the wearer uses the
room: a thumb held on a button, a slow scroll), takes the reports the compositor would have taken
at the glasses' 72 Hz, and for each pad:

* **rests** -- half a second or more in which the thumb stayed within a hundredth of a pad of
  where it was: how far it wandered (side to side, the larger axis), and how often and how far
  the held position moved after the band had had 150 ms to settle;
* **swings** -- on each axis, how often it turned back before going 0.02 of a pad from its last
  turn, as reported and as held: the back and forth that is a shake, whether the thumb rests or
  moves slowly. 2026-11-08, five minutes of the owner in the room: 1,490 and 1,550 on the right
  pad, 22 and 22 held; 1,264 up and down on the left, 219 held.

The band is the one in `crates/spatiand-input/src/steady.rs`; keep the two the same.

    analyse-pad-rest.py trace.csv [--moving 0.004 --resting 0.008 --rest-after 0.1]
"""
import csv, sys

FRAME = 1 / 72


def args():
    a = sys.argv[1:]
    opts = {"--moving": 0.004, "--resting": 0.008, "--rest-after": 0.1}
    for k in list(opts):
        if k in a:
            i = a.index(k)
            opts[k] = float(a[i + 1])
            del a[i : i + 2]
    if not a:
        sys.exit(__doc__)
    return a[0], opts["--moving"], opts["--resting"], opts["--rest-after"]


def frames(path):
    """The last report before each frame's deadline, as the compositor's once-a-frame poll sees it."""
    rows = list(csv.DictReader(open(path)))
    out, next_t, last = [], None, None
    for r in rows:
        t = float(r["t"])
        if next_t is None:
            next_t = t
        while t >= next_t:
            if last is not None:
                out.append((next_t, last))
            next_t += FRAME
        last = r
    return out


def pad(r, side):
    p = side[0]
    return (
        int(r[f"{p}touch"]) == 1,
        int(r[f"{p}click"]) == 1,
        int(r[f"{p}x"]) / 32768.0,
        int(r[f"{p}y"]) / 32768.0,
    )


class Steady:
    """`SteadyPad`, line for line."""

    def __init__(self, moving, resting, rest_after):
        self.moving, self.resting, self.rest_after = moving, resting, rest_after
        self.held = None

    def update(self, touched, x, y, t):
        if not touched:
            self.held = None
            return x, y
        if self.held is None:
            self.held = [x, y, self.moving, t]
            return x, y
        h = self.held
        band = h[2]

        def follow(i, v):
            off = v - h[i]
            moved = abs(off) - band
            if moved <= 0:
                return 0.0
            h[i] += moved if off > 0 else -moved
            return moved

        moved = max(follow(0, x), follow(1, y))
        if moved > 0:
            h[2] = max(band - moved, self.moving)
            h[3] = t
        elif t - h[3] >= self.rest_after:
            h[2] = self.resting
        return h[0], h[1]


def main():
    path, moving, resting, rest_after = args()
    fr = frames(path)
    print(f"{len(fr)} frames ({len(fr) * FRAME:.0f} s) at 72 Hz; band {moving} moving, {resting} resting after {rest_after * 1000:.0f} ms")
    for side in ("left", "right"):
        steady = Steady(moving, resting, rest_after)
        seq = []
        for t, r in fr:
            touched, clicked, x, y = pad(r, side)
            hx, hy = steady.update(touched, x, y, t)
            seq.append((t, touched, clicked, x, y, hx, hy))
        # Rests: windows of 0.5 s or more, touched, unclicked, within 0.01 of their start.
        rests, i, n = [], 0, len(seq)
        while i < n:
            if not seq[i][1] or seq[i][2]:
                i += 1
                continue
            j = i
            while j + 1 < n and seq[j + 1][1] and not seq[j + 1][2] and max(abs(seq[j + 1][3] - seq[i][3]), abs(seq[j + 1][4] - seq[i][4])) < 0.01:
                j += 1
            if (j - i + 1) * FRAME >= 0.5:
                rests.append((i, j))
                i = j + 1
            else:
                i += 1
        wander, moves, moved_far, rest_frames = [], 0, 0.0, 0
        for i, j in rests:
            xs = [s[3] for s in seq[i : j + 1]]
            ys = [s[4] for s in seq[i : j + 1]]
            wander.append(max(max(xs) - min(xs), max(ys) - min(ys)))
            settle = i + int(0.15 / FRAME)
            for k in range(settle + 1, j + 1):
                rest_frames += 1
                d = max(abs(seq[k][5] - seq[k - 1][5]), abs(seq[k][6] - seq[k - 1][6]))
                if d > 0:
                    moves += 1
                    moved_far = max(moved_far, d)
        wander.sort()
        q = lambda a, f: a[min(len(a) - 1, int(f * len(a)))] if a else float("nan")
        print(f"\n{side} pad: {sum(1 for s in seq if s[1])} frames touched, {len(rests)} rests of 0.5 s or more ({sum(j - i + 1 for i, j in rests) * FRAME:.1f} s)")
        if rests:
            print(f"  a resting thumb wandered (side to side): median {q(wander, .5):.4f}, p90 {q(wander, .9):.4f}, most {wander[-1]:.4f} of a pad")
            print(f"  held, after 150 ms: moved on {moves} of {rest_frames} frames, at most {moved_far:.4f}")
        # Swings: an axis turning back before it has gone 0.02 from its last turn.
        def swings(col):
            count, sizes, last, run, prev = 0, [], 0, 0.0, None
            for s in seq:
                if not s[1] or s[2]:
                    last, run, prev = 0, 0.0, None
                    continue
                if prev is not None and s[col] != prev:
                    d = 1 if s[col] > prev else -1
                    if last and d != last:
                        if run < 0.02:
                            count += 1
                            sizes.append(run)
                        run = 0.0
                    run += abs(s[col] - prev)
                    last = d
                prev = s[col]
            sizes.sort()
            return count, (sizes[len(sizes) // 2] if sizes else 0.0)
        for name, raw_col in (("up and down", 4), ("sideways", 3)):
            (rn, rs), (hn, hs) = swings(raw_col), swings(raw_col + 2)
            print(f"  swings {name}: {rn} as reported (median {rs:.4f}), {hn} as held (median {hs:.4f})")


if __name__ == "__main__":
    main()
