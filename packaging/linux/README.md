# Linux packaging

Only KDE Plasma 6 and GNOME on Wayland are supported today. CI builds all of this in
`.github/workflows/linux-packages.yml`; the commands below reproduce it locally. Replace
`~/.cache/ezd-pkg` with any scratch directory on a disk (not a RAM-backed `/tmp`).

## Files

| Path | What it is |
| --- | --- |
| `70-ezdisplay-uinput.rules` | udev rule that lets the logged-in user create uinput devices (touch and pen input). |
| `com.librestatic.ezdisplay.desktop`, `.svg` | Desktop entry and icon. |
| `arch/PKGBUILD`, `arch/ezdisplay.install` | Arch package built from this checkout (no source download). |
| `debian/postinst` | Maintainer script of the `.deb`: reloads udev and re-triggers `/dev/uinput`. The package metadata is `[package.metadata.deb]` in `crates/ezd-tray/Cargo.toml`. |
| `appimage/build.sh`, `pins.env`, `smoke.sh` | AppImage build (FFmpeg and x264 built from pinned sources, adb bundled), its pinned downloads and a smoke test. |

## Arch

```sh
cd packaging/linux/arch
makepkg -sf            # on Arch; installs the build dependencies with -s
sudo pacman -U ezdisplay-*.pkg.tar.zst
```

`makepkg` writes `src/`, `pkg/` and the package next to the `PKGBUILD`; set `BUILDDIR`,
`PKGDEST` and `SRCDEST` to keep the checkout clean. It cannot run as root. From Docker:

```sh
docker run --rm -v "$PWD:/src:ro" -v ~/.cache/ezd-pkg/arch:/work archlinux:latest bash -c '
  pacman -Syu --noconfirm --needed base-devel sudo rust clang pkgconf ffmpeg pipewire libva
  useradd -m builder && chmod 777 /work && cd /src/packaging/linux/arch
  sudo -u builder env BUILDDIR=/work/build PKGDEST=/work/out SRCDEST=/work/src makepkg --noconfirm -f'
```

## Debian and Ubuntu

One `.deb` per distro, because the dependencies are resolved against that distro's FFmpeg
(`$auto` from `dpkg-shlibdeps`). Build inside the matching image:

```sh
docker run --rm -v "$PWD:/src:ro" -v ~/.cache/ezd-pkg/deb:/work -e CARGO_HOME=/work/cargo \
  -e CARGO_TARGET_DIR=/work/target ubuntu:24.04 bash -c '
  apt-get update && apt-get install -y --no-install-recommends build-essential clang libclang-dev \
    pkg-config ca-certificates git curl dpkg-dev libavcodec-dev libavformat-dev libavfilter-dev \
    libavutil-dev libswscale-dev libswresample-dev libpipewire-0.3-dev libva-dev libx11-dev \
    libxkbcommon-dev libwayland-dev
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal && . /work/cargo/env
  cd /src && cargo build --release --locked -p ezd-host -p ezd-tray
  cargo install cargo-deb --version 3.8.0 --locked
  cargo deb -p ezd-tray --no-build --strip --deb-revision 1~ubuntu24.04 --output /work/'
```

`--strip` is needed because the release profile keeps debug info, which cargo-deb would otherwise
leave in the binaries. Use `1~debian13` on `debian:trixie` and `1~ubuntu26.04` on `ubuntu:26.04`. The revision suffix
keeps the three files distinct and makes each sort below a later plain revision. `adb` is a hard
dependency; `xdg-desktop-portal-kde` and the package that ships `kscreen-doctor` are recommended
(KDE only; GNOME needs nothing beyond its own session). On Arch they are optional dependencies.

## AppImage

Built on Ubuntu 24.04 (glibc 2.39 is the floor), with FFmpeg 8.1 built from source so it works the
same on every distro. Needs no libav* development package; see the header of `build.sh` for the
package list. The Rust toolchain must be on the `PATH`.

```sh
docker run --rm -v "$PWD:/src:ro" -v ~/.cache/ezd-pkg/appimage:/work -e CARGO_HOME=/work/cargo \
  -e CARGO_TARGET_DIR=/work/target -e DEBIAN_FRONTEND=noninteractive ubuntu:24.04 bash -c '
  apt-get update && apt-get install -y --no-install-recommends build-essential clang libclang-dev \
    pkg-config nasm curl ca-certificates git xz-utils unzip file patchelf binutils libva-dev \
    libdrm-dev libpipewire-0.3-dev libvulkan-dev libxkbcommon-dev libwayland-dev libx11-dev \
    glslang-tools glslang-dev spirv-tools
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal && . /work/cargo/env
  /src/packaging/linux/appimage/build.sh --out /work/out --cache /work/downloads --work /work/build'
```

Downloads are pinned with SHA-256 in `appimage/pins.env`; the build fails on a mismatch, and a
cached file with the right hash is not downloaded again. The result is
`out/ezdisplay-<version>-x86_64.AppImage` plus `out/ezdisplay-appimage-sources/` (the GPL
sources, see [docs/releasing.md](../../docs/releasing.md)).

The build refuses to produce an AppImage whose binaries need any library outside the bundle other
than glibc, libgcc_s, libva, libva-drm, libdrm, libvulkan and PipeWire, or glibc symbols newer than
2.39. Smoke test on a machine without FFmpeg (needs `jq`):

```sh
packaging/linux/appimage/smoke.sh out/ezdisplay-*.AppImage
```

Inside the image, `AppRun` starts the tray app; `AppRun host ...` runs `ezd-host`. The bundled adb
is at `usr/bin/adb/adb` and the licence texts at `usr/bin/licenses/`, where the app looks for them.
