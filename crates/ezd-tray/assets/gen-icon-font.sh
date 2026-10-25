#!/bin/sh
# Regenerates fonts/fluent-icons-subset.ttf: the glyphs of src/ui/glyphs.rs, cut out of Microsoft's
# Fluent System Icons (MIT, see packaging/windows/licenses/FluentSystemIcons-LICENSE.txt).
# Needs curl and uvx. When you add a glyph, add its codepoint to UNICODES and a constant to glyphs.rs.
set -eu
cd "$(dirname "$0")"

TAG=1.1.344
URL=https://github.com/microsoft/fluentui-system-icons/raw/$TAG/fonts/FluentSystemIcons-Regular.ttf

# ic_fluent_<name>_20_regular, in the order of glyphs.rs.
UNICODES=E9DC,F359,E8D7,E999,F6A9,E4AC,F605,F72A,F294,F2A3,F2B0,F2B6,F869,F4A3,F3F1,F298,F2DD,F42E,F582,EE86,F190,F779,F609,F33F,F4C7,ED88,F18D,ED9B,F84C,E443,F378

tmp=$(mktemp -d "${TMPDIR:-/tmp}/ezd-fonts.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$URL" -o "$tmp/FluentSystemIcons-Regular.ttf"
uvx --from fonttools pyftsubset "$tmp/FluentSystemIcons-Regular.ttf" \
    --unicodes="$UNICODES" --no-layout-closure --layout-features='' \
    --output-file=fonts/fluent-icons-subset.ttf
