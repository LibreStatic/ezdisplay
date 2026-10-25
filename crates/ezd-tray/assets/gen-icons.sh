#!/bin/sh
# Regenerates the committed icon outputs from icons/*.svg. Needs rsvg-convert and magick.
#   status PNGs  -> status-<name>-32.png / -64.png   (tray icons)
#   ezdisplay.ico -> exe icon and installer SetupIconFile (16/24/32/48/64/256)
#   launcher SVG  -> packaging/linux/com.librestatic.ezdisplay.svg
set -eu
cd "$(dirname "$0")"

for pair in stopped:glyph waiting:badge-waiting streaming:badge-streaming error:badge-error; do
    name=${pair%%:*}
    src=${pair##*:}
    for size in 32 64; do
        rsvg-convert -w "$size" -h "$size" "icons/$src.svg" -o "status-$name-$size.png"
    done
done

tmp=$(mktemp -d "${TMPDIR:-/tmp}/ezd-ico.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
for size in 16 24 32 48 64 256; do
    rsvg-convert -w "$size" -h "$size" icons/launcher.svg -o "$tmp/$size.png"
done
magick "$tmp/16.png" "$tmp/24.png" "$tmp/32.png" "$tmp/48.png" "$tmp/64.png" "$tmp/256.png" ezdisplay.ico

cp icons/launcher.svg ../../../packaging/linux/com.librestatic.ezdisplay.svg
