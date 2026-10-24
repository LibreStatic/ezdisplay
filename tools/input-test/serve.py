#!/usr/bin/env python3
"""Serves the input test page and appends the events it posts to a JSONL file.

    serve.py [--port 8765] [--out events.jsonl]   serve and log
    serve.py --summary events.jsonl               summarize a log and exit
"""

import argparse
import collections
import http.server
import json
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_DIR = Path.home() / ".cache" / "ezd-input-test"


def serve(port, out):
    out.parent.mkdir(parents=True, exist_ok=True)
    lock = threading.Lock()
    log = out.open("a", encoding="utf-8")

    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *a, **kw):
            super().__init__(*a, directory=str(HERE), **kw)

        def do_POST(self):
            if self.path != "/log":
                self.send_error(404)
                return
            body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            now = time.time() * 1000
            lines = []
            for line in body.decode("utf-8", "replace").splitlines():
                try:
                    rec = json.loads(line)
                except ValueError:
                    continue
                rec["srv"] = round(now, 3)
                lines.append(json.dumps(rec, separators=(",", ":")))
            with lock:
                log.write("".join(l + "\n" for l in lines))
                log.flush()
            self.send_response(204)
            self.end_headers()

        def log_message(self, fmt, *args):
            if self.command != "POST":
                super().log_message(fmt, *args)

    httpd = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
    print(f"http://127.0.0.1:{port}/  logging to {out}", flush=True)
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        log.close()


def rng(values):
    return f"{min(values)}..{max(values)}" if values else "-"


def summary(path):
    counts = collections.Counter()
    pen_contact, pen_hover = [], []
    tilt_x, tilt_y, twist = [], [], []
    buttons = collections.Counter()
    times = collections.defaultdict(list)
    lag = collections.defaultdict(list)
    nx, ny = collections.defaultdict(list), collections.defaultdict(list)
    touch_down, max_touch = set(), 0
    wheel = {"n": 0, "dx": 0.0, "dy": 0.0, "ctrl": 0}
    geometry = None
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        try:
            r = json.loads(line)
        except ValueError:
            continue
        t, kind = r.get("type"), r.get("kind")
        counts[(t, kind)] += 1
        if t == "geometry":
            geometry = r
        elif t == "wheel":
            wheel["n"] += 1
            wheel["dx"] += r["dx"]
            wheel["dy"] += r["dy"]
            wheel["ctrl"] += bool(r["ctrl"])
        elif t and t.startswith("pointer"):
            times[kind].append(r["t"])
            lag[kind].append(r["lag"])
            nx[kind].append(r["nx"])
            ny[kind].append(r["ny"])
            buttons[(kind, r["buttons"])] += 1
            if kind == "pen":
                (pen_contact if r["buttons"] & 1 else pen_hover).append(r["pressure"])
                tilt_x.append(r["tiltX"])
                tilt_y.append(r["tiltY"])
                twist.append(r["twist"])
            if kind == "touch":
                if t == "pointerdown":
                    touch_down.add(r["id"])
                    max_touch = max(max_touch, len(touch_down))
                elif t in ("pointerup", "pointercancel"):
                    touch_down.discard(r["id"])

    if geometry:
        g = geometry
        print(f"window {g['innerWidth']}x{g['innerHeight']} @{g['dpr']} at "
              f"({g['screenX']}, {g['screenY']}), screen {g['screenW']}x{g['screenH']}")
    print("events:")
    for (t, kind), n in sorted(counts.items(), key=lambda kv: (str(kv[0][1]), str(kv[0][0]))):
        print(f"  {kind or '-':6} {t:14} {n}")
    for kind, ts in sorted(times.items()):
        span = (max(ts) - min(ts)) / 1000
        moves = counts[("pointermove", kind)]
        per_s = f"{moves / span:.0f} moves/s" if span > 0 else ""
        lags = sorted(lag[kind])
        p50 = lags[len(lags) // 2]
        p95 = lags[min(len(lags) - 1, len(lags) * 95 // 100)]
        print(f"{kind}: {per_s}, dispatch lag p50 {p50} ms p95 {p95} ms, "
              f"x {rng(nx[kind])}, y {rng(ny[kind])}")
    if pen_contact or pen_hover:
        print(f"pen pressure in contact {rng(pen_contact)}, in hover {rng(pen_hover)}")
        print(f"pen tiltX {rng(tilt_x)}, tiltY {rng(tilt_y)}, twist {rng(twist)}")
    if buttons:
        print("buttons: " + ", ".join(f"{k}={b}:{n}" for (k, b), n in sorted(buttons.items())))
    if max_touch:
        print(f"touch: max {max_touch} simultaneous contacts")
    if wheel["n"]:
        print(f"wheel: {wheel['n']} events, dx {wheel['dx']:.0f}, dy {wheel['dy']:.0f}, "
              f"with ctrl {wheel['ctrl']}")
    for t in ("contextmenu", "auxclick", "scroll"):
        if counts[(t, None)]:
            print(f"{t}: {counts[(t, None)]}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--out", type=Path, help=f"JSONL file (default: {DEFAULT_DIR}/<time>.jsonl)")
    ap.add_argument("--summary", metavar="FILE", help="summarize a log instead of serving")
    a = ap.parse_args()
    if a.summary:
        summary(a.summary)
        return
    out = a.out or DEFAULT_DIR / time.strftime("%Y%m%d-%H%M%S.jsonl")
    serve(a.port, out)


if __name__ == "__main__":
    sys.exit(main())
