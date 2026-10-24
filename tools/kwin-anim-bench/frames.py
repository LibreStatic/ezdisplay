# usage: python3 -I frames.py <host log> <events> <kind> [n]  -- per-frame timeline (ms) after the first n events of a kind:
# KWin frames, encode stages and encoded frames
import re, sys
from datetime import datetime
ts = lambda s: datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp()
rows = []
for l in open(sys.argv[1], errors="replace"):
    l = re.sub(r"\x1b\[[0-9;]*m", "", l)
    if not l.startswith("20"): continue
    if "encode stages" in l or ("TRACE" in l and " encoded " in l) or "pipewire frame" in l:
        d = dict((k, int(v)) for k, v in re.findall(r"(\w+)=(\d+)", l))
        rows.append((ts(l.split()[0]), "S" if "stages" in l else "E" if "encoded" in l else "P", d))
ev = [(ts(l.split()[0]), m.group(1)) for l in open(sys.argv[2]) for m in [re.search(r"EZDANIM (\S+) \d+", l)] if m]
n = int(sys.argv[4]) if len(sys.argv) > 4 else 2
for t, k in [e for e in ev if e[1] == sys.argv[3]][:n]:
    for s, kind, d in rows:
        if not (t <= s < t + 0.35): continue
        at = (s - t) * 1e3
        if kind == "P": print("%6.1f  KWin frame%s" % (at, " (replaced an unencoded one)" if "replaced" in d or "true" in str(d) else ""))
        elif kind == "S": print("%6.1f    fence %5.1f  convert %5.1f  wait %5.1f  encode %5.1f" % (at, d.get("fence_us", 0) / 1e3, (d["graph_us"] + d.get("sync_us", 0)) / 1e3, d.get("wait_us", 0) / 1e3, d["send_us"] / 1e3))
        else: print("%6.1f    encoded, age %5.1f" % (at, d.get("age_us", 0) / 1e3))
    print()
