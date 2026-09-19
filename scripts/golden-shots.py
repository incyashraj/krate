#!/usr/bin/env python3
"""Golden-scene comparisons for what an app draws (IC-743, test 1547).

`krate run --shoot frame.png` paints an app's window through the shared
painter and writes `frame.png.json` beside it: the renderer, the scale, the
pixel and logical sizes and the colour space. A golden is such a shot kept
on purpose, with a tolerance written down. A check shoots the app again and
compares -- and REFUSES to compare two shots that differ in renderer, scale,
colour space or size, because a difference between them would not be a
difference in the app. That refusal is the point of 1547: a screenshot
comparison that does not say what it compared is a number nobody can read.

    golden-shots.py record  APP.krate --golden-dir DIR [--tolerance 0.005] [--scale 2] [--after-ms 400]
    golden-shots.py check   APP.krate --golden-dir DIR [--diff out.png]
    golden-shots.py compare GOLDEN.png FRESH.png [--tolerance F] [--diff out.png]
    golden-shots.py --self-test

Exit codes: 0 the frames match within tolerance; 1 they differ beyond it;
2 the comparison was refused (mismatched renderer, scale, colour space,
size, bundle, or a golden with no tolerance).

The PNG reader and writer here are the standard library's zlib plus the
format's own rules, so the tool has no dependencies to install on a CI
runner: 8-bit RGB and RGBA, non-interlaced, every filter type.
"""

import hashlib
import json
import os
import struct
import subprocess
import sys
import tempfile
import zlib
from pathlib import Path

SIDECAR_SCHEMA = "krate.shot.v1"
GOLDEN_SCHEMA = "krate.golden.v1"
# A pixel differs when any channel moves by more than this. Antialiasing
# at a fractional edge wobbles by a level or two between otherwise
# identical renders; a control that vanished moves by far more.
DEFAULT_CHANNEL_DELTA = 8
DEFAULT_MAX_FRACTION = 0.005
IDENTITY_FIELDS = ("renderer", "scale", "color_space", "width", "height")


# ------------------------------------------------------------------ PNG ----

def _chunk(kind, data):
    body = kind + data
    return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)


def write_png(path, width, height, rows, channels=4, filter_type=0):
    """Write 8-bit RGB(A) rows (each a bytes of width*channels) with one
    filter type on every scanline. The filter is a parameter so a test can
    exercise the reader on all five."""
    color = 6 if channels == 4 else 2
    raw = bytearray()
    prev = bytes(width * channels)
    for row in rows:
        row = bytes(row)
        raw.append(filter_type)
        raw.extend(_filter(filter_type, row, prev, channels))
        prev = row
    png = b"\x89PNG\r\n\x1a\n"
    png += _chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, color, 0, 0, 0))
    png += _chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += _chunk(b"IEND", b"")
    Path(path).write_bytes(png)


def _filter(kind, row, prev, bpp):
    out = bytearray(len(row))
    for i, x in enumerate(row):
        a = row[i - bpp] if i >= bpp else 0
        b = prev[i]
        c = prev[i - bpp] if i >= bpp else 0
        if kind == 0:
            p = 0
        elif kind == 1:
            p = a
        elif kind == 2:
            p = b
        elif kind == 3:
            p = (a + b) // 2
        elif kind == 4:
            p = _paeth(a, b, c)
        else:
            raise ValueError(f"filter {kind}")
        out[i] = (x - p) & 0xFF
    return bytes(out)


def _paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c


def read_png(path):
    """Return (width, height, channels, rows) for an 8-bit RGB/RGBA PNG."""
    data = Path(path).read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path}: not a PNG")
    at = 8
    width = height = channels = None
    idat = bytearray()
    while at + 8 <= len(data):
        length, kind = struct.unpack(">I4s", data[at:at + 8])
        body = data[at + 8:at + 8 + length]
        at += 12 + length
        if kind == b"IHDR":
            width, height, depth, color, _c, _f, interlace = struct.unpack(">IIBBBBB", body)
            if depth != 8:
                raise ValueError(f"{path}: {depth}-bit PNGs are not read here (8-bit only)")
            if color == 6:
                channels = 4
            elif color == 2:
                channels = 3
            else:
                raise ValueError(f"{path}: colour type {color} is not RGB or RGBA")
            if interlace:
                raise ValueError(f"{path}: interlaced PNGs are not read here")
        elif kind == b"IDAT":
            idat.extend(body)
        elif kind == b"IEND":
            break
    if width is None:
        raise ValueError(f"{path}: no IHDR")
    raw = zlib.decompress(bytes(idat))
    stride = width * channels
    rows = []
    prev = bytes(stride)
    at = 0
    for _ in range(height):
        kind = raw[at]
        line = raw[at + 1:at + 1 + stride]
        at += 1 + stride
        row = _unfilter(kind, line, prev, channels)
        rows.append(row)
        prev = row
    return width, height, channels, rows


def _unfilter(kind, line, prev, bpp):
    out = bytearray(len(line))
    for i, x in enumerate(line):
        a = out[i - bpp] if i >= bpp else 0
        b = prev[i]
        c = prev[i - bpp] if i >= bpp else 0
        if kind == 0:
            p = 0
        elif kind == 1:
            p = a
        elif kind == 2:
            p = b
        elif kind == 3:
            p = (a + b) // 2
        elif kind == 4:
            p = _paeth(a, b, c)
        else:
            raise ValueError(f"unknown PNG filter {kind}")
        out[i] = (x + p) & 0xFF
    return bytes(out)


# ------------------------------------------------------------ comparison ----

def sidecar_of(png_path):
    return Path(str(png_path) + ".json")


def load_sidecar(png_path):
    path = sidecar_of(png_path)
    if not path.is_file():
        return None, f"{png_path}: no sidecar ({path.name}); a shot with no record of what painted it cannot be compared (1547)"
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError) as err:
        return None, f"{path}: unreadable sidecar: {err}"
    if data.get("schema") != SIDECAR_SCHEMA:
        return None, f"{path}: sidecar schema {data.get('schema')!r} is not {SIDECAR_SCHEMA}"
    return data, None


def refuse_reason(golden, fresh):
    """Why two sidecars must not be compared, or None."""
    for field in IDENTITY_FIELDS:
        g, f = golden.get(field), fresh.get(field)
        if g != f:
            return f"refused: the golden's {field} is {g!r} and the fresh shot's is {f!r}; a difference between them is not a difference in the app (1547)"
    return None


def compare_pixels(golden_png, fresh_png, channel_delta, diff_path=None):
    """Return (differing_fraction, differing_count, total, bbox)."""
    gw, gh, gc, grows = read_png(golden_png)
    fw, fh, fc, frows = read_png(fresh_png)
    if (gw, gh) != (fw, fh):
        raise ValueError(f"refused: the PNGs are {gw}x{gh} and {fw}x{fh}; sizes must match to compare pixels (1547)")
    channels = min(gc, fc)
    total = gw * gh
    differing = 0
    bbox = None
    diff_rows = [] if diff_path else None
    for y in range(gh):
        g, f = grows[y], frows[y]
        diff_row = bytearray(gw * 4) if diff_rows is not None else None
        for x in range(gw):
            gi, fi = x * gc, x * fc
            moved = False
            for ch in range(channels):
                if abs(g[gi + ch] - f[fi + ch]) > channel_delta:
                    moved = True
                    break
            if moved:
                differing += 1
                if bbox is None:
                    bbox = [x, y, x, y]
                else:
                    bbox[0] = min(bbox[0], x)
                    bbox[1] = min(bbox[1], y)
                    bbox[2] = max(bbox[2], x)
                    bbox[3] = max(bbox[3], y)
                if diff_row is not None:
                    diff_row[x * 4:x * 4 + 4] = b"\xff\x00\x00\xff"
            elif diff_row is not None:
                v = g[gi] // 3 + 40
                diff_row[x * 4:x * 4 + 4] = bytes((v, v, v, 255))
        if diff_rows is not None:
            diff_rows.append(bytes(diff_row))
    if diff_rows is not None:
        write_png(diff_path, gw, gh, diff_rows, 4)
    return (differing / total if total else 0.0), differing, total, bbox


def compare(golden_png, fresh_png, tolerance=None, diff_path=None):
    """The whole comparison: sidecars first, then pixels under the golden's
    tolerance (or the one given). Returns (exit_code, message)."""
    golden, why = load_sidecar(golden_png)
    if why:
        return 2, why
    fresh, why = load_sidecar(fresh_png)
    if why:
        return 2, why
    reason = refuse_reason(golden, fresh)
    if reason:
        return 2, reason
    declared = golden.get("golden") or {}
    if tolerance is None:
        tol = (declared.get("tolerance") or {}).get("max_differing_fraction")
        if tol is None:
            return 2, "refused: the golden declares no tolerance; a comparison must say how much may differ before it is run (1547)"
    else:
        tol = tolerance
    delta = (declared.get("tolerance") or {}).get("channel_delta", DEFAULT_CHANNEL_DELTA)
    try:
        fraction, count, total, bbox = compare_pixels(golden_png, fresh_png, delta, diff_path)
    except ValueError as err:
        return 2, str(err)
    where = f" in the box x={bbox[0]}..{bbox[2]} y={bbox[1]}..{bbox[3]}" if bbox else ""
    summary = (
        f"{count} of {total} pixels differ by more than {delta} in a channel "
        f"({fraction:.4%}); the tolerance is {tol:.4%}; renderer {golden['renderer']} at "
        f"{golden['scale']}x, {golden['width']}x{golden['height']}, {golden['color_space']}{where}"
    )
    if fraction <= tol:
        return 0, "match -- " + summary
    return 1, "DIFFERENT -- " + summary


# ------------------------------------------------------- shoot and record ----

def shoot(krate, bundle, out_png, scale, after_ms, frame_step_ms=None):
    """Take one shot. `frame_step_ms` puts the run on a stepped clock: the app
    sees each drawn frame as exactly that many milliseconds long and the delay
    is counted in those frames, so the pose it is caught in does not depend on
    how fast this machine happened to be.

    That dependency is K-721. `cubes` advances its spin by real elapsed time,
    so at 400 real milliseconds it had turned a different amount on every run
    and the macOS lane went red on about half of all pushes with no code change
    behind it. Ten shots differed from each other by up to 1.1%, against a
    0.5% tolerance. On a stepped clock all ten are byte-identical, and stay so
    under heavy CPU load."""
    env = dict(os.environ)
    if after_ms:
        env["KRATE_SHOOT_AFTER_MS"] = str(after_ms)
    cmd = shoot_command(krate, bundle, out_png, scale, frame_step_ms)
    proc = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=300)
    if not Path(out_png).is_file():
        raise RuntimeError(f"no shot was written by {' '.join(cmd)} (exit {proc.returncode}):\n{proc.stderr[-2000:]}")
    if not sidecar_of(out_png).is_file():
        raise RuntimeError(f"the runtime wrote {out_png} but no sidecar beside it; this Krate predates krate.shot.v1")
    return proc.returncode


def shoot_command(krate, bundle, out_png, scale, frame_step_ms=None):
    """The argv one shot runs. Split out so a self-test can check that the
    stepped clock actually reaches the runtime -- a shot that quietly fell back
    to the host clock would look reproducible in this tool and stay random in
    CI, which is K-721 coming back with nothing to show it had."""
    cmd = [krate, "run", "--shoot", str(out_png), "--shoot-scale", str(scale), "--auto-grant", str(bundle)]
    if frame_step_ms:
        cmd[4:4] = ["--frame-step-millis", str(frame_step_ms)]
    return cmd


def bundle_sha256(bundle):
    return hashlib.sha256(Path(bundle).read_bytes()).hexdigest()


def golden_paths(golden_dir, bundle):
    stem = Path(bundle).stem
    png = Path(golden_dir) / f"{stem}.png"
    return png, sidecar_of(png)


def record(args):
    golden_dir = Path(args.golden_dir)
    golden_dir.mkdir(parents=True, exist_ok=True)
    png, sidecar = golden_paths(golden_dir, args.bundle)
    with tempfile.TemporaryDirectory() as tmp:
        shot_png = Path(tmp) / "shot.png"
        shoot(args.krate, args.bundle, shot_png, args.scale, args.after_ms, args.frame_step_ms)
        data, why = load_sidecar(shot_png)
        if why:
            print(why, file=sys.stderr)
            return 2
        data["golden"] = {
            "schema": GOLDEN_SCHEMA,
            "bundle_sha256": bundle_sha256(args.bundle),
            "bundle": Path(args.bundle).name,
            "tolerance": {"max_differing_fraction": args.tolerance, "channel_delta": args.channel_delta},
            "after_ms": args.after_ms,
            # The clock the golden was taken on, so a check uses the same one.
            # A golden shot on the host clock and re-shot on a stepped one is
            # two different poses of an animated app, which is not a
            # comparison -- the same reason this tool refuses to compare two
            # different renderers or scales.
            "frame_step_ms": args.frame_step_ms,
            "commit": _head(),
        }
        png.write_bytes(shot_png.read_bytes())
        sidecar.write_text(json.dumps(data, indent=1) + "\n")
    print(f"recorded {png} ({data['width']}x{data['height']}, {data['renderer']} at {data['scale']}x) with tolerance {args.tolerance:.4%}")
    return 0


def check(args):
    png, sidecar = golden_paths(args.golden_dir, args.bundle)
    if not png.is_file() or not sidecar.is_file():
        print(f"refused: no golden for {Path(args.bundle).name} in {args.golden_dir}; record one first", file=sys.stderr)
        return 2
    golden = json.loads(sidecar.read_text())
    declared = golden.get("golden") or {}
    if declared.get("bundle_sha256") and declared["bundle_sha256"] != bundle_sha256(args.bundle):
        print("refused: the golden was recorded from a different bundle than the one being checked; re-record it for this app (1547)", file=sys.stderr)
        return 2
    if "frame_step_ms" not in declared:
        # A golden recorded before the stepped clock (K-721) was taken on the
        # host clock, so it is re-shot on the host clock -- comparing it against
        # a stepped shot would compare two different poses of an animated app,
        # which is not a comparison at all.
        #
        # So this is not a refusal, it is the old behaviour, kept working, with
        # a note that the old behaviour is the unreliable one. Each host records
        # its own goldens (K-337), so the macOS set moved to the stepped clock
        # on the machine that could re-record it and the others move when
        # theirs can. Until then those hosts keep the flakiness they already
        # had -- which on Linux and Windows measured 0.001-0.01% against a
        # 0.5% tolerance, so it has never actually bitten them.
        print(
            f"note: the golden for {Path(args.bundle).name} predates the stepped screenshot clock, "
            "so this check re-shoots on the host clock and an animated app may differ from run to "
            "run (K-721); re-record it on this host to make it reproducible",
            file=sys.stderr,
        )
    with tempfile.TemporaryDirectory() as tmp:
        shot_png = Path(tmp) / "shot.png"
        shoot(
            args.krate,
            args.bundle,
            shot_png,
            golden.get("scale", 2.0),
            declared.get("after_ms", 0),
            declared.get("frame_step_ms"),
        )
        code, message = compare(png, shot_png, diff_path=args.diff)
    print(message, file=sys.stderr if code else sys.stdout)
    return code


def _head():
    try:
        out = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, timeout=10)
        return out.stdout.strip() or "unknown"
    except (OSError, subprocess.SubprocessError):
        return "unknown"


# ---------------------------------------------------------------- self-test --

def self_test():
    failures = []

    def check_(name, cond, detail=""):
        if not cond:
            failures.append(f"{name}: {detail}" if detail else name)

    def sidecar(**over):
        base = {"schema": SIDECAR_SCHEMA, "renderer": "krate-shared-painter", "scale": 2.0,
                "width": 40, "height": 30, "logical_width": 20, "logical_height": 15,
                "color_space": "sRGB 8-bit RGBA, straight alpha, from an ARGB8888 framebuffer",
                "os": "macos", "arch": "aarch64", "runtime": "0.4.0"}
        base.update(over)
        return base

    def scene(width, height, seed=0):
        rows = []
        for y in range(height):
            row = bytearray()
            for x in range(width):
                row.extend(((x * 7 + seed) & 0xFF, (y * 5) & 0xFF, ((x ^ y) * 3) & 0xFF, 255))
            rows.append(bytes(row))
        return rows

    with tempfile.TemporaryDirectory() as tmp:
        d = Path(tmp)
        w, h = 40, 30
        base = scene(w, h)

        # The reader survives every filter type the encoder may pick.
        for ft in range(5):
            p = d / f"f{ft}.png"
            write_png(p, w, h, base, 4, filter_type=ft)
            rw, rh, rc, rows = read_png(p)
            check_(f"filter {ft} round-trips", (rw, rh, rc) == (w, h, 4) and rows == base)
        # RGB without alpha reads too.
        rgb = [bytes(b for i, b in enumerate(r) if i % 4 != 3) for r in base]
        write_png(d / "rgb.png", w, h, rgb, 3, 2)
        check_("RGB reads", read_png(d / "rgb.png")[2] == 3)

        gold = d / "gold.png"
        write_png(gold, w, h, base, 4, 4)
        sidecar_of(gold).write_text(json.dumps(dict(sidecar(), golden={"tolerance": {"max_differing_fraction": 0.02, "channel_delta": 8}})))

        # Identical: match.
        same = d / "same.png"
        write_png(same, w, h, base, 4, 1)
        sidecar_of(same).write_text(json.dumps(sidecar()))
        code, msg = compare(gold, same)
        check_("identical frames match", code == 0, msg)

        # Antialiasing wobble (every channel +3): still a match.
        wobble = [bytes(min(255, b + 3) if i % 4 != 3 else b for i, b in enumerate(r)) for r in base]
        wob = d / "wobble.png"
        write_png(wob, w, h, wobble, 4, 3)
        sidecar_of(wob).write_text(json.dumps(sidecar()))
        code, msg = compare(gold, wob)
        check_("a channel wobble under the delta is not a difference", code == 0, msg)

        # 1% of pixels changed hard, tolerance 2%: match, and the count is right.
        one = [bytearray(r) for r in base]
        for k in range(12):  # 12 of 1200 pixels = 1%
            x, y = (k * 3) % w, (k * 7) % h
            one[y][x * 4:x * 4 + 3] = b"\xff\xff\xff"
        p1 = d / "one.png"
        write_png(p1, w, h, [bytes(r) for r in one], 4, 0)
        sidecar_of(p1).write_text(json.dumps(sidecar()))
        code, msg = compare(gold, p1)
        check_("1% under a 2% tolerance matches", code == 0 and "12 of 1200" in msg, msg)

        # 5% changed: different, and the message says how much and where.
        five = [bytearray(r) for r in base]
        for k in range(60):
            x, y = (k * 3) % w, (k * 7) % h
            five[y][x * 4:x * 4 + 3] = b"\x00\x00\x00"
        p5 = d / "five.png"
        write_png(p5, w, h, [bytes(r) for r in five], 4, 2)
        sidecar_of(p5).write_text(json.dumps(sidecar()))
        diff = d / "diff.png"
        code, msg = compare(gold, p5, diff_path=diff)
        check_("5% over a 2% tolerance differs", code == 1 and "DIFFERENT" in msg and "tolerance is 2.0000%" in msg, msg)
        check_("the message locates the difference", "in the box" in msg, msg)
        check_("a diff image is written", diff.is_file() and read_png(diff)[0] == w)

        # Refusals: each identity field on its own (1547).
        for field, other in [("renderer", "vello-gpu"), ("scale", 1.0),
                             ("color_space", "linear RGBA"), ("width", 41)]:
            p = d / f"mismatch-{field}.png"
            write_png(p, w, h, base, 4, 0)
            sidecar_of(p).write_text(json.dumps(sidecar(**{field: other})))
            code, msg = compare(gold, p)
            check_(f"a {field} mismatch is refused, not compared", code == 2 and field in msg and "1547" in msg, msg)
        # A size mismatch in the PNG itself, even with a lying sidecar.
        small = d / "small.png"
        write_png(small, w - 1, h, [r[:-4] for r in base], 4, 0)
        sidecar_of(small).write_text(json.dumps(sidecar()))
        code, msg = compare(gold, small)
        check_("a PNG whose size disagrees is refused", code == 2 and "sizes must match" in msg, msg)
        # No sidecar, no comparison.
        bare = d / "bare.png"
        write_png(bare, w, h, base, 4, 0)
        code, msg = compare(gold, bare)
        check_("a shot with no sidecar is refused", code == 2 and "no sidecar" in msg, msg)
        # A golden with no tolerance is refused.
        notol = d / "notol.png"
        write_png(notol, w, h, base, 4, 0)
        sidecar_of(notol).write_text(json.dumps(sidecar()))
        code, msg = compare(notol, same)
        check_("a golden with no tolerance is refused", code == 2 and "no tolerance" in msg, msg)
        code, msg = compare(notol, same, tolerance=0.01)
        check_("unless one is given on the command line", code == 0, msg)

        # The stepped clock reaches the runtime (K-721). A shot that fell back
        # to the host clock would still look fine here and stay random in CI,
        # so the flag is asserted rather than assumed.
        stepped = shoot_command("krate", "a.krate", "o.png", 2.0, 16)
        check_(
            "a stepped shot passes --frame-step-millis to the runtime",
            "--frame-step-millis" in stepped and stepped[stepped.index("--frame-step-millis") + 1] == "16",
            " ".join(stepped),
        )
        check_(
            "the step does not disturb the rest of the argv",
            [a for a in stepped if a not in ("--frame-step-millis", "16")]
            == shoot_command("krate", "a.krate", "o.png", 2.0, None),
            " ".join(stepped),
        )
        check_(
            "and a step of 0 or None leaves the host clock alone",
            "--frame-step-millis" not in shoot_command("krate", "a.krate", "o.png", 2.0, 0)
            and "--frame-step-millis" not in shoot_command("krate", "a.krate", "o.png", 2.0, None),
        )

    if failures:
        print("golden-shots self-test FAILED:\n")
        for f in failures:
            print(f"  - {f}")
        return 1
    print("golden-shots self-test OK -- the PNG reader survives every filter, a wobble under the channel delta is not a difference, a change is counted and located, and a comparison across renderer, scale, colour space, size or without a tolerance is refused, and a stepped shot really passes --frame-step-millis")
    return 0


def main(argv):
    import argparse
    if "--self-test" in argv:
        return self_test()
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("record")
    r.add_argument("bundle")
    r.add_argument("--golden-dir", required=True)
    r.add_argument("--krate", default=os.environ.get("KRATE_BIN", "target/release/krate"))
    r.add_argument("--scale", type=float, default=2.0)
    r.add_argument("--after-ms", type=int, default=400)
    # On by default: an app that does not read the clock is unaffected, and an
    # app that does is the only kind this tool ever compared unreliably.
    r.add_argument("--frame-step-ms", type=int, default=16,
                   help="milliseconds of app-visible time per drawn frame; 0 uses the host clock")
    r.add_argument("--tolerance", type=float, default=DEFAULT_MAX_FRACTION)
    r.add_argument("--channel-delta", type=int, default=DEFAULT_CHANNEL_DELTA)
    c = sub.add_parser("check")
    c.add_argument("bundle")
    c.add_argument("--golden-dir", required=True)
    c.add_argument("--krate", default=os.environ.get("KRATE_BIN", "target/release/krate"))
    c.add_argument("--diff")
    m = sub.add_parser("compare")
    m.add_argument("golden")
    m.add_argument("fresh")
    m.add_argument("--tolerance", type=float)
    m.add_argument("--diff")
    args = ap.parse_args(argv)
    if args.cmd == "record":
        return record(args)
    if args.cmd == "check":
        return check(args)
    code, message = compare(args.golden, args.fresh, args.tolerance, args.diff)
    print(message, file=sys.stderr if code else sys.stdout)
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
