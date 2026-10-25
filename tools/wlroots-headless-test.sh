#!/bin/sh
# Runs the wlroots backend tests (`sway::tests::headless_sway_encoded_frames` and
# `wl_capture::tests::headless_sway_frames`) against a headless sway inside Docker, so sway does not
# have to be installed on the host. Hyprland is not covered: it needs a GPU or a render node to start.
#
#   tools/wlroots-headless-test.sh                   # ext-image-copy-capture (plus cursor session)
#   EZD_WL_CAPTURE=wlr tools/wlroots-headless-test.sh   # wlr-screencopy only
#
# The first run builds the image `ezd-wlroots-test` (sway, rust, ffmpeg and the build dependencies).
# Build output goes to $EZD_WLROOTS_CACHE (default ~/.cache/ezdisplay/wlroots-target): keep it off
# tmpfs. Remove the image with `docker rmi ezd-wlroots-test` when done.
set -eu

if [ -z "${EZD_WLROOTS_INNER:-}" ]; then
    repo=$(cd "$(dirname "$0")/.." && pwd)
    image=${EZD_WLROOTS_IMAGE:-ezd-wlroots-test}
    cache=${EZD_WLROOTS_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/ezdisplay/wlroots-target}
    mkdir -p "$cache"
    if ! docker image inspect "$image" >/dev/null 2>&1; then
        docker build -t "$image" - <<'DOCKERFILE'
FROM archlinux:latest
RUN pacman -Syu --noconfirm --needed sway wayland-utils rust clang pkgconf base-devel ffmpeg pipewire \
        libva mesa vulkan-swrast libdrm wayland wayland-protocols libxkbcommon \
    && pacman -Scc --noconfirm
DOCKERFILE
    fi
    exec docker run --rm \
        --user "$(id -u):$(id -g)" \
        -v "$repo":/src:ro \
        -v "$cache":/target \
        -v "$HOME/.cargo/registry":/cargo/registry \
        -e CARGO_HOME=/cargo -e CARGO_TARGET_DIR=/target -e HOME=/tmp/home \
        -e EZD_WLROOTS_INNER=1 -e EZD_WL_CAPTURE="${EZD_WL_CAPTURE:-}" -e RUST_LOG="${RUST_LOG:-info}" \
        -w /src "$image" sh /src/tools/wlroots-headless-test.sh "$@"
fi

# Inside the container.
mkdir -p "$HOME/bin" "$HOME/run"
chmod 700 "$HOME/run"
export XDG_RUNTIME_DIR="$HOME/run"
# sway ships with file capabilities that a container without CAP_SETFCAP cannot grant: use a copy.
cp /usr/bin/sway "$HOME/bin/sway"
cargo test -p ezd-host-linux --lib --offline --locked --no-run
WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 "$HOME/bin/sway" >"$HOME/sway.log" 2>&1 &
sway_pid=$!
trap 'kill $sway_pid 2>/dev/null || true' EXIT
for _ in $(seq 50); do
    SWAYSOCK=$(ls "$XDG_RUNTIME_DIR"/sway-ipc.*.sock 2>/dev/null | head -n 1 || true)
    [ -n "$SWAYSOCK" ] && break
    sleep 0.2
done
export SWAYSOCK
WAYLAND_DISPLAY=$(cd "$XDG_RUNTIME_DIR" && ls | grep '^wayland-[0-9]*$' | head -n 1)
export WAYLAND_DISPLAY
echo "$("$HOME/bin/sway" --version) on $WAYLAND_DISPLAY; capture protocols advertised:"
wayland-info | grep -E "interface: '(ext_image_copy|ext_output_image|zwlr_screencopy|zwp_linux_dmabuf|wl_shm|zwlr_virtual_pointer|wl_seat)" || true
EZD_WLROOTS_TEST=1 cargo test -p ezd-host-linux --lib --offline --locked headless_sway \
    "$@" -- --nocapture --test-threads=1
