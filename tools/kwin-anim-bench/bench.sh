#!/bin/bash
# KWin animation benchmark for the ezdisplay Linux host (KDE Plasma 6). See README.md.
#
#   bench.sh <label> [ezd-host run args...]   host run on the tablet's virtual output
#   bench.sh --panel <output> <label>         the same animations on a real output, no host
set -u
# Keep the screen on: KWin stops drawing while the panel is blanked.
if [ -z "${EZD_BENCH_INHIBITED:-}" ] && command -v kde-inhibit >/dev/null; then
    EZD_BENCH_INHIBITED=1 exec kde-inhibit --power --screenSaver "$0" "$@"
fi
here=$(cd "$(dirname "$0")" && pwd)
usage() {
    echo "usage: $0 [--panel OUTPUT] <label> [ezd-host run args...]" >&2
    exit 2
}
panel=
if [ "${1:-}" = --panel ]; then
    panel=${2:-}
    [ -n "$panel" ] || usage
    shift 2
fi
[ $# -ge 1 ] || usage
label=$1
shift
out=${EZD_BENCH_OUT:-${XDG_CACHE_HOME:-$HOME/.cache}/ezd-bench}
host=${EZD_HOST:-$here/../../target/release/ezd-host}
editor=${EZD_BENCH_EDITOR:-kwrite}
export WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-wayland-0} XDG_CURRENT_DESKTOP=${XDG_CURRENT_DESKTOP:-KDE}
export XDG_SESSION_TYPE=wayland
mkdir -p "$out"
log=$out/$label.log events=$out/$label.events gpu=$out/$label.gpu
rm -f "$log" "$events" "$gpu"

hostpid=
if [ -z "$panel" ]; then
    serial=${ANDROID_SERIAL:?set ANDROID_SERIAL to the tablet serial}
    [ -x "$host" ] || { echo "no ezd-host at $host (set EZD_HOST)" >&2; exit 1; }
    app=com.librestatic.ezdisplay
    adb -s "$serial" shell am force-stop $app
    (sleep 2; adb -s "$serial" shell am start -n $app/.MainActivity >/dev/null) &
    RUST_LOG=${EZD_LOG:-info,ezd_host_linux::source=trace,ezd_host_linux::encode=trace,ezd_host_linux::pw=trace} \
        timeout -s INT -k 5 50 "$host" run --source virtual --adb "$serial" "$@" > "$log" 2>&1 &
    hostpid=$!
    sleep 10
    target=Virtual
else
    target=$panel
fi

# GPU busy sampler (Intel only: reads RC6 residency).
gpupid=
if python3 -I "$here/gpu.py" card >/dev/null 2>&1; then
    python3 -I "$here/gpu.py" sample "$gpu" 38 &
    gpupid=$!
fi

# The window the script maximizes and moves; its caption must contain EZD-DRAG.
seq 1 200 | sed 's/^/ezdisplay animation benchmark line /' > "$out/EZD-DRAG.txt"
$editor "$out/EZD-DRAG.txt" >/dev/null 2>&1 &
edpid=$!
sleep 3

q=$(mktemp -d "$out/q-XXXXXX")
sed "s/@OUTPUT@/$target/" "$here/anim.qml" > "$q/main.qml"
since=$(date "+%F %T")
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.unloadScript ezd-anim >/dev/null 2>&1
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.loadDeclarativeScript "$q/main.qml" ezd-anim >/dev/null
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.start
sleep 32
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.unloadScript ezd-anim >/dev/null
kill $edpid 2>/dev/null
rm -rf "$q"
journalctl --user --since "$since" -o short-iso-precise | grep -E "EZDANIM|EZDTICK" > "$events"
[ -n "$gpupid" ] && wait $gpupid
if [ -n "$hostpid" ]; then
    wait $hostpid
    adb -s "$serial" shell am force-stop $app
fi

if grep -q "EZDANIM no-output" "$events"; then
    echo "no output named $target* found" >&2
    exit 1
fi
grep -q "EZDANIM maximize" "$events" || { echo "the EZD-DRAG window never appeared" >&2; exit 1; }
[ -n "$hostpid" ] && python3 -I "$here/report.py" "$log" "$events"
[ -s "$gpu" ] && python3 -I "$here/gpu.py" report "$gpu" "$events"
python3 -I "$here/tick.py" "$events"
