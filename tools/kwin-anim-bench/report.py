# usage: python3 -I report.py <host log> <events>  -- per animation kind: frames from KWin, frames encoded, gaps, slow conversions; client e2e
import re, sys, statistics as st
from datetime import datetime
log, evf = sys.argv[1], sys.argv[2]
ts = lambda s: datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp()
enc = []; stages = []; client = []; pwf = []
for l in open(log, errors="replace"):
    l = re.sub(r"\x1b\[[0-9;]*m", "", l)
    if not l.startswith("20"): continue
    t = ts(l.split()[0]); d = dict((k, int(v)) for k, v in re.findall(r"(\w+)=(\d+)", l))
    if "encode stages" in l: stages.append((t, d.get("fence_us", 0) + d["graph_us"] + d.get("sync_us", 0)))
    elif "TRACE" in l and " encoded " in l: enc.append(t)
    elif "pipewire frame" in l: pwf.append(t)
    elif "client stats" in l and d.get("recv"): client.append((t, d))
ev = []
for l in open(evf):
    m = re.search(r"EZDANIM (\S+) (\d+)", l)
    if m: ev.append((ts(l.split()[0]), m.group(1)))
W = 0.5
rows = {}
for i, (t, kind) in enumerate(ev):
    if kind in ("move-start", "end"): continue
    out = [e for e in enc if t <= e < t + W]
    pts = [t] + out
    gaps = [b - a for a, b in zip(pts, pts[1:])]
    first = (out[0] - t) * 1e3 if out else W * 1e3
    worst = max(gaps) * 1e3 if out else W * 1e3
    stall = sum(1 for s, w in stages if t <= s < t + W and w > 40000)
    pin = [e for e in pwf if t <= e < t + W]
    pp = [t] + pin
    pgap = max(b - a for a, b in zip(pp, pp[1:])) * 1e3 if pin else W * 1e3
    rows.setdefault(kind, []).append((len(out), first, worst, stall, len(pin), pgap))
def show(kind, r):
    print("%-13s x%d | from KWin %4.1f (gap %5.1f ms) | encoded %4.1f | first after %5.1f ms | longest gap %5.1f ms (worst %5.1f) | conversions > 40 ms %d" % (
        kind, len(r), st.median(x[4] for x in r), st.median(x[5] for x in r), st.median(x[0] for x in r), st.median(x[1] for x in r), st.median(x[2] for x in r), max(x[2] for x in r), sum(x[3] for x in r)))
for k in ("outline-show", "outline-hide", "maximize", "restore"):
    if k in rows: show(k, rows[k])
ms = [t for t, k in ev if k == "move-start"]; me = [t for t, k in ev if k == "end"]
if ms and me:
    dur = me[0] - ms[0]
    for name, src in (("move KWin", pwf), ("move encoded", enc)):
        out = [e for e in src if ms[0] <= e < me[0]]
        gaps = sorted(b - a for a, b in zip(out, out[1:]))
        if gaps: print("%-13s %.1f s | %.0f frames/s | gap p50 %.1f p95 %.1f max %.1f ms" % (name, dur, len(out) / dur, st.median(gaps) * 1e3, gaps[int(len(gaps) * .95)] * 1e3, gaps[-1] * 1e3))
if ev:
    c = [d for t, d in client if ev[0][0] <= t <= ev[-1][0] + 1]
    if c: print("client during test: e2e p50 (median of 1 s windows) %.1f ms, p95 %.1f ms, decode p50 %.1f ms" % (st.median(x["e2e_p50_us"] for x in c) / 1e3, st.median(x["e2e_p95_us"] for x in c) / 1e3, st.median(x["decode_p50_us"] for x in c) / 1e3))
