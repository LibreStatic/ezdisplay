# Releasing

A release is a `vX.Y.Z` tag. The *Release* workflow builds every package from that commit and
publishes them in one GitHub Release.

## Steps

1. Bump `version` in `[workspace.package]` of the root `Cargo.toml` (every crate follows it) and run
   `cargo check --workspace` so `Cargo.lock` is updated. Merge that to `main`.
2. Optional dry run: *Actions, Release, Run workflow* on `main` with `dry_run` ticked (the default).
   It runs the whole pipeline and uploads the files as the `release-preview` artifact, without
   creating a release.
3. Tag and push:

   ```sh
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

   Tags containing `-` (for example `v0.2.0-rc1`) are published as pre-releases. The workflow
   fails at once if the tag does not match the workspace version.

## What the workflow does

| Job | What it does |
| --- | --- |
| `version` | Reads the workspace version and checks it against the tag. |
| `ci` | Calls `ci.yml`: fmt, clippy and tests on Linux, the Windows build and installer (`ezdisplay-windows-installer`). |
| `android` | Calls `android.yml`: the APKs (`ezdisplay-android-apks`). |
| `linux` | Calls `linux-packages.yml`: the Arch package (`ezdisplay-arch`), one `.deb` per distro (`ezdisplay-deb-ubuntu24.04`, `-debian13`, `-ubuntu26.04`), the AppImage (`ezdisplay-appimage`) and its sources (`ezdisplay-appimage-sources`), plus a smoke test of the AppImage on a clean Debian 13. |
| `publish` | Runs only when all of the above pass. Collects the release files, writes `SHA256SUMS` and creates the release with generated notes (or uploads `release-preview` on a dry run). |

The release assets are:

- `ezdisplay-setup-<version>-x64.exe` (Windows installer);
- `ezdisplay-<version>-1-x86_64.pkg.tar.zst` (Arch);
- `ezdisplay_<version>-1~ubuntu24.04_amd64.deb`, `..~debian13_amd64.deb`, `..~ubuntu26.04_amd64.deb`;
- `ezdisplay-<version>-x86_64.AppImage`;
- `ezdisplay-<version>-appimage-sources.tar` (see below);
- `ezdisplay-<version>-full-debug.apk` and `ezdisplay-<version>-play-debug.apk`;
- `SHA256SUMS`.

The same workflows also run on every push to `main` and on pull requests; the artifacts of those
runs are the way to try a build before tagging.

## GPL obligations of the AppImage

The AppImage bundles FFmpeg and x264 built with `--enable-gpl`, so the combined work it ships is
under the GPL. ezdisplay itself stays MIT or Apache-2.0. To comply:

- The exact sources of everything bundled (FFmpeg, x264, plus the headers used to build them), the
  build script and the pins are published next to the AppImage as
  `ezdisplay-<version>-appimage-sources.tar`, and as the `ezdisplay-appimage-sources` artifact.
  Always publish that file in the same release as the AppImage, and keep it for as long as the
  AppImage is available.
- The AppImage carries the licence texts and a notice with the version and configure line in
  `usr/bin/licenses/` (the app's licence view reads it from there).
- When you change `packaging/linux/appimage/pins.env` or the FFmpeg configure line in `build.sh`,
  the sources tarball changes with them: nothing else to update by hand.
