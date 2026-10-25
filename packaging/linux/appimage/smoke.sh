#!/bin/sh
# Smoke test of the AppImage on a machine WITHOUT FFmpeg: the bundled one must be used.
# Needs: jq and ldd. Usage: smoke.sh <ezdisplay-*.AppImage>
set -eu

image=$(realpath "$1")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
[ -x "$image" ] || chmod +x "$image"
cd "$work"

echo "== host --version"
"$image" --appimage-extract-and-run host --version

echo "== host diag (failures to find a GPU are fine, the output must be JSON)"
# Redirect to a file: diag starts the adb server, which would keep a pipe open.
timeout 120 "$image" --appimage-extract-and-run host diag > diag.json
jq -e . diag.json > /dev/null
head -c 600 diag.json
echo

echo "== extracted binaries use the bundled FFmpeg"
"$image" --appimage-extract > /dev/null
for bin in ezd-host ezdisplay; do
    deps=$(ldd "squashfs-root/usr/bin/$bin")
    if echo "$deps" | grep -q 'not found'; then
        echo "$deps" | grep 'not found'
        echo "ERROR: unresolved libraries in $bin" >&2
        exit 1
    fi
    echo "$deps" | grep -E 'libav|libsw' | sed 's#/usr/bin/\.\./lib/#/usr/lib/#' \
        | grep -q "$work/squashfs-root/usr/lib/" \
        || { echo "ERROR: $bin does not use the bundled FFmpeg" >&2; exit 1; }
    echo "$deps" | grep -E 'libav|libsw'
done
echo "smoke test passed"
