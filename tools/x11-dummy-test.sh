#!/bin/sh
# Runs the X11 backend tests (`x11_test.rs`) against an Xorg with the dummy video driver, so no
# desktop session or Docker is needed. Unlike Xvfb, the dummy driver uses xf86 modes, which is what
# reproduces the server's pointer constraint to the CRTC areas that the backend has to lift.
#
#   tools/x11-dummy-test.sh                 # all gated tests
#   tools/x11-dummy-test.sh x11_capture     # only tests whose name matches (default: x11)
#
# Needs `Xorg` and `xf86-video-dummy` (Arch: xorg-server xf86-video-dummy; Debian/Ubuntu:
# xserver-xorg-core xserver-xorg-video-dummy). Scratch files (config, server log) go to
# $EZD_X11_CACHE (default ~/.cache/ezdisplay/x11-test); the Rust build uses the normal target
# directory. The server runs on the first free display number from :90.
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
cache=${EZD_X11_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/ezdisplay/x11-test}
mkdir -p "$cache"

xorg=${XORG:-}
if [ -z "$xorg" ]; then
    for c in /usr/lib/Xorg /usr/lib/xorg/Xorg /usr/libexec/Xorg "$(command -v Xorg || true)"; do
        if [ -n "$c" ] && [ -x "$c" ]; then
            xorg=$c
            break
        fi
    done
fi
[ -n "$xorg" ] || { echo "Xorg not found (set XORG=/path/to/Xorg)" >&2; exit 1; }

cat >"$cache/dummy.conf" <<'CONF'
Section "Device"
  Identifier "d"
  Driver "dummy"
  VideoRam 256000
EndSection
Section "Monitor"
  Identifier "m"
  HorizSync 5.0-1000.0
  VertRefresh 5.0-200.0
  Modeline "1920x1080" 148.50 1920 2008 2052 2200 1080 1084 1089 1125
EndSection
Section "Screen"
  Identifier "s"
  Device "d"
  Monitor "m"
  DefaultDepth 24
  SubSection "Display"
    Depth 24
    Modes "1920x1080"
    Virtual 8192 4096
  EndSubSection
EndSection
CONF

# Build first, so a compile error does not leave a server running.
cd "$repo"
cargo test -p ezd-host-linux --lib --locked --no-run

display=
n=90
while [ "$n" -lt 190 ]; do
    if [ ! -e "/tmp/.X11-unix/X$n" ] && [ ! -e "/tmp/.X$n-lock" ]; then
        display=:$n
        break
    fi
    n=$((n + 1))
done
[ -n "$display" ] || { echo "no free X display number" >&2; exit 1; }

"$xorg" "$display" -config "$cache/dummy.conf" -configdir "$cache" -logfile "$cache/Xorg.log" \
    -noreset -nolisten tcp -novtswitch -sharevts >"$cache/Xorg.out" 2>&1 &
xorg_pid=$!
trap 'kill $xorg_pid 2>/dev/null || true; wait $xorg_pid 2>/dev/null || true' EXIT

# Wait for the socket, then for the server to answer.
for _ in $(seq 50); do
    [ -e "/tmp/.X11-unix/X${display#:}" ] && break
    kill -0 "$xorg_pid" 2>/dev/null || { echo "Xorg exited; see $cache/Xorg.log" >&2; cat "$cache/Xorg.out" >&2; exit 1; }
    sleep 0.2
done
export DISPLAY=$display
echo "Xorg on $DISPLAY (log: $cache/Xorg.log)"
if command -v xrandr >/dev/null 2>&1; then xrandr --listmonitors || true; fi

[ "$#" -gt 0 ] || set -- x11
EZD_X11_TEST=1 cargo test -p ezd-host-linux --lib --locked "$@" -- --nocapture --test-threads=1
