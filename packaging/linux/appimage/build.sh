#!/usr/bin/env bash
# Builds the ezdisplay AppImage: a minimal GPL FFmpeg (with x264 and the hardware paths the host
# uses) built from pinned sources, the two binaries linked against it, the bundled adb, and the
# AppImage itself.
#
# Run it inside an ubuntu:24.04 container (glibc 2.39 is the floor of the result). Supported
# targets: any x86_64 distro with glibc 2.39 or newer and KDE Plasma 6 on Wayland (KDE neon,
# Debian 13, Fedora 40+, Arch, openSUSE Tumbleweed, Kubuntu 24.10+). libva, libdrm, libvulkan and
# PipeWire come from the host; everything else is bundled.
#
# The container needs (and must NOT have any libav*-dev package, so the system FFmpeg can never
# be picked up):
#   build-essential clang libclang-dev pkg-config nasm curl ca-certificates git xz-utils unzip
#   file patchelf binutils libva-dev libdrm-dev libpipewire-0.3-dev libvulkan-dev libxkbcommon-dev
#   libwayland-dev libx11-dev glslang-tools glslang-dev spirv-tools
#
# Usage: build.sh [--out DIR] [--cache DIR] [--work DIR] [--jobs N]
#   --out    where the AppImage and the sources bundle go (default: ./out)
#   --cache  download cache; a file whose SHA-256 matches its pin is not downloaded again
#            (default: <work>/downloads)
#   --work   scratch directory for sources, builds and the AppDir (default: ./work)
#   --jobs   parallel jobs (default: nproc)
# Environment: CARGO_TARGET_DIR (default: <repo>/target), RUSTFLAGS (extra flags are kept).
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)

OUT=$PWD/out
WORK=$PWD/work
CACHE=
JOBS=$(nproc)
while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT=$(realpath -m "$2"); shift 2 ;;
        --cache) CACHE=$(realpath -m "$2"); shift 2 ;;
        --work) WORK=$(realpath -m "$2"); shift 2 ;;
        --jobs) JOBS=$2; shift 2 ;;
        -h|--help) sed -n '2,/^set -e/p' "$0" | sed '$d'; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
CACHE=${CACHE:-$WORK/downloads}

log() { printf '\n==> %s\n' "$*"; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

# shellcheck source=pins.env
. "$HERE/pins.env"

mkdir -p "$OUT" "$WORK" "$CACHE"
PREFIX=$WORK/prefix
SRC=$WORK/src
APPDIR=$WORK/AppDir
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$REPO/target}

# --- 1. Downloads ---------------------------------------------------------------------------

# fetch NAME: puts the pinned file in $CACHE (reusing a cached copy whose hash matches).
fetch() {
    local name=$1 url file sha
    url=$(eval "echo \${${name}_URL}")
    file=$(eval "echo \${${name}_FILE}")
    sha=$(eval "echo \${${name}_SHA256}")
    if [ -f "$CACHE/$file" ] && [ "$(sha256sum "$CACHE/$file" | cut -d' ' -f1)" = "$sha" ]; then
        echo "cached: $file"
        return
    fi
    local dl="$WORK/dl-$name"
    rm -rf "$dl"
    mkdir -p "$dl"
    echo "downloading: $url"
    curl --fail --location --silent --show-error --retry 3 --output "$dl/$file" "$url"
    local got
    got=$(sha256sum "$dl/$file" | cut -d' ' -f1)
    [ "$got" = "$sha" ] || die "SHA-256 mismatch for $file: expected $sha, got $got"
    mv "$dl/$file" "$CACHE/$file"
    rmdir "$dl"
}

log "Downloading pinned sources and tools"
for n in FFMPEG X264 NVCODEC VULKAN APPIMAGETOOL RUNTIME ADB; do fetch $n; done

# unpack NAME DIR: extracts the archive into an empty DIR (one top-level directory is stripped).
unpack() {
    local file dest=$2
    file=$(eval "echo \${${1}_FILE}")
    rm -rf "$dest"
    mkdir -p "$dest"
    tar -xf "$CACHE/$file" -C "$dest" --strip-components=1
}

# --- 2. Headers ---------------------------------------------------------------------------

log "Installing nv-codec-headers and unpacking Vulkan-Headers"
rm -rf "$PREFIX"
mkdir -p "$PREFIX"
unpack NVCODEC "$SRC/nv-codec-headers"
make -C "$SRC/nv-codec-headers" install PREFIX="$PREFIX" >/dev/null
unpack VULKAN "$SRC/vulkan-headers"

# --- 3. x264 (8-bit, static, PIC) ---------------------------------------------------------

log "Building x264"
unpack X264 "$SRC/x264"
(
    cd "$SRC/x264"
    ./configure --prefix="$PREFIX" --enable-static --enable-pic --disable-cli \
        --disable-opencl --disable-avs --disable-swscale --disable-lavf --disable-ffms \
        --disable-gpac --disable-lsmash --bit-depth=8
    make -j"$JOBS"
    make install
)

# --- 4. FFmpeg ----------------------------------------------------------------------------

log "Building FFmpeg $FFMPEG_VERSION"
unpack FFMPEG "$SRC/ffmpeg"
export PKG_CONFIG_PATH=$PREFIX/lib/pkgconfig
FFMPEG_CONFIGURE=(
    --prefix="$PREFIX"
    --enable-shared --disable-static --enable-pic
    --enable-gpl --enable-libx264
    --disable-autodetect --disable-everything --disable-programs --disable-doc
    --disable-avdevice --disable-avformat --disable-swresample --disable-network
    --disable-xlib
    --enable-encoder=h264_vaapi,h264_nvenc,libx264
    --enable-decoder=h264
    --enable-parser=h264
    --enable-filter=buffer,buffersink,format,null,scale,hwmap,hwupload,scale_vaapi,scale_vulkan
    --enable-vaapi --enable-libdrm --enable-vulkan
    --enable-ffnvcodec --enable-nvenc
    # scale_vulkan compiles its shaders at run time: glslang is linked statically (with
    # libstdc++) into libavfilter, so the AppImage needs no extra shared library.
    --enable-libglslang
    --extra-cflags="-I$SRC/vulkan-headers/include"
    --extra-ldflags="-static-libstdc++ -static-libgcc"
)
(
    cd "$SRC/ffmpeg"
    ./configure "${FFMPEG_CONFIGURE[@]}"
    make -j"$JOBS"
    make install
)
# Every FFmpeg library finds its siblings next to itself.
for so in "$PREFIX"/lib/lib*.so.*.*; do
    [ -L "$so" ] || patchelf --set-rpath '$ORIGIN' "$so"
done

# --- 5. The binaries ----------------------------------------------------------------------

log "Building ezd-host and ezdisplay"
[ "$(pkg-config --variable=prefix libavcodec)" = "$PREFIX" ] \
    || die "pkg-config does not resolve libavcodec to the bundled FFmpeg ($PREFIX)"
export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-rpath,\$ORIGIN/../lib"
(cd "$REPO" && cargo build --release --locked -p ezd-host -p ezd-tray)
VERSION=$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p}' "$REPO/Cargo.toml")
[ -n "$VERSION" ] || die "cannot read the workspace version from Cargo.toml"
echo "ezdisplay $VERSION"

# --- 6. AppDir ----------------------------------------------------------------------------

log "Assembling the AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin/adb" "$APPDIR/usr/bin/licenses" "$APPDIR/usr/lib" \
    "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/scalable/apps"
# The release profile keeps debug info; the shipped binaries do not need it.
install -s -m755 "$CARGO_TARGET_DIR/release/ezdisplay" "$CARGO_TARGET_DIR/release/ezd-host" "$APPDIR/usr/bin/"

# FFmpeg libraries: one real file per soname, no development symlinks.
for lib in avcodec avfilter avutil swscale; do
    real=$(readlink -f "$PREFIX/lib/lib$lib.so")
    soname=$(objdump -p "$real" | awk '/SONAME/ {print $2}')
    install -s -m755 "$real" "$APPDIR/usr/lib/$soname"
done

# adb from the Android SDK Platform-Tools (found by the app at <exe dir>/adb/adb).
rm -rf "$SRC/platform-tools"
mkdir -p "$SRC/platform-tools"
unzip -q -o "$CACHE/$ADB_FILE" -d "$SRC/platform-tools"
install -m755 "$SRC/platform-tools/platform-tools/adb" "$APPDIR/usr/bin/adb/adb"
install -m644 "$SRC/platform-tools/platform-tools/NOTICE.txt" "$APPDIR/usr/bin/adb/NOTICE.txt"
if [ -d "$SRC/platform-tools/platform-tools/lib64" ]; then
    mkdir -p "$APPDIR/usr/bin/adb/lib64"
    install -m755 "$SRC/platform-tools/platform-tools/lib64/"* "$APPDIR/usr/bin/adb/lib64/"
fi

# Licenses (the app reads them from <exe dir>/licenses).
L=$APPDIR/usr/bin/licenses
install -m644 "$REPO/LICENSE-MIT" "$REPO/LICENSE-APACHE" "$L/"
install -m644 "$REPO/packaging/windows/licenses/FluentSystemIcons-LICENSE.txt" "$L/"
install -m644 "$SRC/ffmpeg/COPYING.GPLv2" "$SRC/ffmpeg/COPYING.GPLv3" "$SRC/ffmpeg/LICENSE.md" "$L/"
install -m644 "$SRC/x264/COPYING" "$L/x264-COPYING"
for pkg in glslang-dev glslang-tools spirv-tools; do
    [ -f "/usr/share/doc/$pkg/copyright" ] && install -m644 "/usr/share/doc/$pkg/copyright" "$L/$pkg-copyright"
done
FF_CONFIG_LINE="./configure ${FFMPEG_CONFIGURE[*]}"
cat > "$L/FFmpeg-x264-NOTICE.txt" <<NOTICE
ezdisplay AppImage: bundled third-party software
================================================

FFmpeg $FFMPEG_VERSION (libavcodec, libavfilter, libavutil, libswscale)
  Licence: GNU GPL v2 or later (built with --enable-gpl; see COPYING.GPLv2, COPYING.GPLv3 and
  LICENSE.md next to this file). Source: $FFMPEG_URL
  Configure line:
    $FF_CONFIG_LINE

x264 $X264_VERSION (statically linked into libavcodec)
  Licence: GNU GPL v2 or later (see x264-COPYING). Source: $X264_URL

glslang and SPIRV-Tools (statically linked into libavfilter, for scale_vulkan)
  Licences: see the glslang and spirv-tools copyright files next to this file.

nv-codec-headers $NVCODEC_VERSION (MIT) and Vulkan-Headers $VULKAN_VERSION (Apache-2.0 or MIT):
  build-time headers only, nothing of them is shipped.

Android SDK Platform-Tools $ADB_VERSION (adb): see usr/bin/adb/NOTICE.txt.

Corresponding source
  The exact sources of everything above, and the script and pins used to build this AppImage
  (build.sh, pins.env), are published next to every release as the asset
  ezdisplay-<version>-appimage-sources.tar, and as the CI artifact ezdisplay-appimage-sources.
  Rebuild with: packaging/linux/appimage/build.sh (see packaging/linux/README.md).

ezdisplay itself is dual licensed under MIT or Apache-2.0 (LICENSE-MIT, LICENSE-APACHE).
NOTICE

install -m644 "$REPO/packaging/linux/com.librestatic.ezdisplay.desktop" "$APPDIR/usr/share/applications/"
install -m644 "$REPO/packaging/linux/com.librestatic.ezdisplay.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/"
install -m644 "$REPO/packaging/linux/com.librestatic.ezdisplay.desktop" "$APPDIR/"
install -m644 "$REPO/packaging/linux/com.librestatic.ezdisplay.svg" "$APPDIR/"
ln -s com.librestatic.ezdisplay.svg "$APPDIR/.DirIcon"
cat > "$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
# `AppRun host ...` runs the ezd-host command line tool; anything else starts the tray app.
HERE=$(dirname "$(readlink -f "$0")")
if [ "${1:-}" = "host" ]; then
    shift
    exec "$HERE/usr/bin/ezd-host" "$@"
fi
exec "$HERE/usr/bin/ezdisplay" "$@"
APPRUN
chmod 755 "$APPDIR/AppRun"

# --- 7. Dependency gate -------------------------------------------------------------------

log "Checking the dependencies of every ELF in the AppDir"
ALLOW='^(libc\.so\.6|libm\.so\.6|libdl\.so\.2|libpthread\.so\.0|librt\.so\.1|libmvec\.so\.1|libgcc_s\.so\.1|ld-linux-x86-64\.so\.2|libva\.so\.2|libva-drm\.so\.2|libdrm\.so\.2|libvulkan\.so\.1|libpipewire-0\.3\.so\.0)$'
MAX_GLIBC=2.39
gate_failed=0
while IFS= read -r -d '' f; do
    file -b "$f" | grep -q '^ELF' || continue
    deps=$(ldd "$f" 2>&1 || true)
    case "$deps" in *"not a dynamic executable"*) continue ;; esac
    echo "$deps" | grep -q 'not found' && {
        echo "GATE: $f has unresolved libraries:" >&2
        echo "$deps" | grep 'not found' >&2
        gate_failed=1
    }
    while read -r name path; do
        case "$path" in "$APPDIR"/*) continue ;; esac
        if ! echo "$name" | grep -Eq "$ALLOW"; then
            echo "GATE: $f needs $name ($path), which is neither bundled nor allowed" >&2
            gate_failed=1
        fi
    done < <(echo "$deps" | awk '/=> \// {print $1, $3} /^\t\// {n=$1; sub(".*/", "", n); print n, $1}')
    max=$(objdump -T "$f" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -V | tail -1)
    if [ -n "$max" ] && [ "$(printf '%s\n%s\n' "$max" "$MAX_GLIBC" | sort -V | tail -1)" != "$MAX_GLIBC" ]; then
        echo "GATE: $f needs GLIBC_$max, newer than $MAX_GLIBC" >&2
        gate_failed=1
    fi
    echo "ok: ${f#"$APPDIR"/} (glibc ${max:-none})"
done < <(find "$APPDIR" -type f -print0)
[ "$gate_failed" = 0 ] || die "dependency gate failed (see GATE lines above)"

# --- 8. AppImage and sources --------------------------------------------------------------

log "Packing the AppImage"
install -m755 "$CACHE/$APPIMAGETOOL_FILE" "$WORK/appimagetool"
IMAGE=$OUT/ezdisplay-$VERSION-x86_64.AppImage
rm -f "$IMAGE"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$WORK/appimagetool" \
    --runtime-file "$CACHE/$RUNTIME_FILE" --no-appstream "$APPDIR" "$IMAGE"

log "Collecting the corresponding sources"
SRCOUT=$OUT/ezdisplay-appimage-sources
rm -rf "$SRCOUT"
mkdir -p "$SRCOUT"
for n in FFMPEG X264 NVCODEC VULKAN; do
    install -m644 "$CACHE/$(eval "echo \${${n}_FILE}")" "$SRCOUT/"
done
install -m755 "$HERE/build.sh" "$SRCOUT/"
install -m644 "$HERE/pins.env" "$SRCOUT/"
install -m644 "$L/FFmpeg-x264-NOTICE.txt" "$SRCOUT/"

ls -l "$IMAGE" "$SRCOUT"
