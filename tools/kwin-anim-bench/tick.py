# usage: python3 -I tick.py <events>  -- longest KWin main-thread stall (late 10 ms timer) after each animation kind
import re, sys, statistics as st
from datetime import datetime
ts = lambda s: datetime.fromisoformat(s.split()[0]).timestamp()
ev, tk = [], []
for l in open(sys.argv[1]):
    m = re.search(r"EZDANIM (\S+) \d+", l)
    if m: ev.append((ts(l), m.group(1)))
    m = re.search(r"EZDTICK (\d+)", l)
    if m: tk.append((ts(l), int(m.group(1))))
rows = {}
for t, k in ev:
    w = [d for s, d in tk if t <= s < t + 0.5]
    rows.setdefault(k, []).append(max(w, default=0))
for k in ("outline-show", "outline-hide", "maximize", "restore"):
    if k in rows: print("%-13s KWin main thread: longest stall median %3.0f ms, worst %3.0f ms (stalls > 30 ms in %d of %d events)" % (k, st.median(rows[k]), max(rows[k]), sum(1 for x in rows[k] if x), len(rows[k])))
