#!/bin/sh
# Runs the GNOME backend test (`gnome::tests::headless_mutter_frames`) against a headless Mutter on a
# private D-Bus session. Keeps XDG_RUNTIME_DIR, so Mutter and the test use the PipeWire daemon of the
# current user session. Needs mutter and a Wayland client that draws continuously (Mutter only sends
# frames when the screen changes); alacritty is used here, set EZD_CLIENT to use another one.
#
#   tools/gnome-headless-test.sh [extra cargo test arguments]
#   EZD_GNOME_TEST_ENCODER=libx264 EZD_NO_DMABUF=1 tools/gnome-headless-test.sh
set -eu
if [ -z "${EZD_GNOME_INNER:-}" ]; then
    exec env EZD_GNOME_INNER=1 dbus-run-session -- "$0" "$@"
fi
display=ezd-test-$$
mutter --headless --wayland --no-x11 --wayland-display "$display" >/dev/null 2>&1 &
mutter_pid=$!
client_pid=
trap 'kill $client_pid $mutter_pid 2>/dev/null || true' EXIT
sleep 3
client="${EZD_CLIENT:-alacritty -e sh -c 'while :; do date +%s.%N; sleep 0.03; done'}"
WAYLAND_DISPLAY=$display sh -c "$client" >/dev/null 2>&1 &
client_pid=$!
sleep 1
EZD_GNOME_TEST=1 cargo test -p ezd-host-linux headless_mutter "$@" -- --nocapture
