# usage: python3 -I gpu.py card | sample <out> <secs> | report <samples> <events>
# Intel iGPU load: busy % = 1 - RC6 (idle) residency, sampled every 10 ms. The frequency file reads
# the idle clock while the GPU sleeps, so RC6 is the meaningful number.
import glob, os, re, statistics as st, sys, time
from datetime import datetime


def card():
    for c in sorted(glob.glob("/sys/class/drm/card[0-9]*")):
        if os.path.exists(c + "/power/rc6_residency_ms") and os.path.exists(c + "/gt_act_freq_mhz"):
            return c
    sys.exit("no Intel GPU with RC6 residency in sysfs")


def sample(out, secs):
    c = card()
    f, r = open(c + "/gt_act_freq_mhz"), open(c + "/power/rc6_residency_ms")
    end = time.time() + secs
    with open(out, "w") as o:
        while time.time() < end:
            f.seek(0)
            r.seek(0)
            o.write("%.4f %s %s\n" % (time.time(), f.read().strip(), r.read().strip()))
            time.sleep(0.01)


def report(samples, events):
    fr = [tuple(map(float, l.split())) for l in open(samples) if len(l.split()) == 3]
    ev = [(datetime.fromisoformat(l.split()[0]).timestamp(), m.group(1))
          for l in open(events) for m in [re.search(r"EZDANIM (\S+) \d+", l)] if m]

    def busy(a, b):
        w = [x for x in fr if a <= x[0] < b]
        if len(w) < 2:
            return None
        idle = (w[-1][2] - w[0][2]) / ((w[-1][0] - w[0][0]) * 1e3)
        return max(0, 1 - idle) * 100

    rows = {}
    for t, k in ev:
        b = busy(t, t + 0.3)
        if b is not None:
            rows.setdefault(k, []).append(b)
    for k in ("outline-show", "outline-hide", "maximize", "restore"):
        if k in rows:
            print("%-13s GPU busy %3.0f %% (300 ms after the event)" % (k, st.median(rows[k])))
    ms = [t for t, k in ev if k == "move-start"]
    if ms and busy(ms[0], ms[0] + 3) is not None:
        print("%-13s GPU busy %3.0f %%" % ("move", busy(ms[0], ms[0] + 3)))


if sys.argv[1:2] == ["card"]:
    print(card())
elif sys.argv[1:2] == ["sample"]:
    sample(sys.argv[2], float(sys.argv[3]))
elif sys.argv[1:2] == ["report"]:
    report(sys.argv[2], sys.argv[3])
else:
    sys.exit(__doc__ or "usage: gpu.py card | sample <out> <secs> | report <samples> <events>")
