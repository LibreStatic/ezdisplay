# ezdisplay

Use an Android tablet as an extended monitor for your PC over USB, with very low latency.

The host (Linux and Windows, Rust) renders a virtual display and encodes it on the same GPU
without copies; the tablet (Kotlin UI + Rust native library) decodes and shows it. The mouse
cursor is sent separately and drawn locally on the tablet, so pointer movement never waits
for a video frame.

## Status

Working today, over USB with ADB:

- extended display from KDE Plasma (Wayland) and Windows, at the tablet's native resolution and
  refresh rate, with the cursor drawn locally;
- touch, pen pressure, tilt and hover, and configurable S Pen button gestures (tested on KDE; the
  Windows input path still needs testing on hardware);
- the PC tray app with a settings and statistics window, and a Windows installer that sets up the
  virtual display driver.

Not yet: the Wi-Fi transport, audio, GNOME and wlroots backends.

| Crate | Purpose |
| --- | --- |
| `ezd-proto` | Wire framing, messages, capability negotiation, clock sync. Pure, builds for Android. |
| `ezd-link` | `Link` byte streams (TCP, in-memory), ADB helper, prioritized mux. |
| `ezd-host-core` | `VideoSource` / `VirtualDisplay` traits, host session, synthetic test frames. |
| `ezd-host-service` | The host loop as a library, with live status for the CLI and the tray app. |
| `ezd-host-linux` | KDE virtual output, PipeWire capture, VAAPI/NVENC encode, uinput input. |
| `ezd-host-windows` | GPU policy, Virtual Display Driver, Desktop Duplication, D3D11 encode, input injection. |
| `ezd-ffmpeg` | Safe FFmpeg wrapper: low-latency `Encoder`, `Decoder`, `SwVideoSource`, encoder probing. |
| `ezd-host` | The host CLI: `run`, `diag` and `vdd`. |
| `ezd-tray` | The PC app (`ezdisplay`): tray icon and menu, settings and statistics window. |
| `ezd-client-core` | Client session shared by the Android library and the fake client. |
| `ezd-input` | Client input engine: palm rejection, S Pen gestures, local zoom. |
| `ezd-android` | Android native library (JNI, AMediaCodec); the app itself is in `android/`. |
| `ezd-fake-client` | Desktop stand-in for the Android client (decodes, measures latency). |

The protocol is specified in [docs/protocol.md](docs/protocol.md) and the design in
[docs/architecture.md](docs/architecture.md).

## USB without developer mode

Besides adb, ezdisplay can talk to the tablet over USB with the Android Open Accessory protocol,
which does not need developer mode or USB debugging.

- **Linux:** needs the udev rule `packaging/linux/70-ezdisplay-usb.rules`, which the packages install
  (from source or an AppImage, install it yourself, see
  [packaging/linux/README.md](packaging/linux/README.md)).
- **Windows:** needs the optional UsbDk driver. The installer offers it as the unchecked task "USB
  without developer mode (installs the UsbDk driver)". Without it, USB falls back to adb.
- The first time, Android asks which app should open the accessory. Pick ezdisplay; choosing "always"
  avoids the question next time.
- In auto mode the host tries the accessory first and falls back to adb. Turn it off with
  `--aoa off` or `aoa = "off"` in `config.toml`.
- A phone or tablet without the app is switched once and then left alone: it is reset back to normal
  mode and not tried again while the host runs.

## Install

**Tablet.** Install the APK (artifact `ezdisplay-android-apks` of the *Android* workflow; there are
no releases yet) and turn on USB debugging.

**Windows.** Run `ezdisplay-setup-<version>-x64.exe` (artifact `ezdisplay-windows-installer` of the
*CI* workflow). It downloads FFmpeg, installs the virtual display driver (Windows asks once
whether to install the device software) and starts ezdisplay in the notification area. Connect the
tablet over USB: the app opens on the tablet and streaming starts. The installer is not signed yet,
so SmartScreen asks for confirmation (*More info*, *Run anyway*) and Smart App Control blocks it.
See [docs/windows-testing.md](docs/windows-testing.md) and
[packaging/windows/README.md](packaging/windows/README.md).

**Linux (KDE Plasma 6, Wayland).** Plasma 6 on Wayland is required; X11 and other desktops are not
supported yet. The packages are in the artifacts of the *Linux packages* workflow and, once there
is a release, in the GitHub Release (see [docs/releasing.md](docs/releasing.md)). Pick one:

- **Arch Linux**: `sudo pacman -U ezdisplay-*.pkg.tar.zst`.
- **Debian and Ubuntu**: `sudo apt install ./ezdisplay_*.deb`, using the file for your release:
  `~ubuntu24.04` (Ubuntu 24.04, KDE neon), `~debian13` (Debian 13 "trixie") or `~ubuntu26.04`
  (Ubuntu 26.04). Kubuntu 24.04 ships Plasma 5.27 and is not supported; KDE neon is.
- **Any other distro (AppImage)**: any x86_64 distro with glibc 2.39 or newer and Plasma 6
  (KDE neon, Debian 13, Fedora 40+, Arch, openSUSE Tumbleweed, Kubuntu 24.10+).

  ```sh
  chmod +x ezdisplay-*.AppImage
  ./ezdisplay-*.AppImage
  ```

  It needs `fusermount3` (fuse3) to mount itself; without it run
  `./ezdisplay-*.AppImage --appimage-extract-and-run`. FFmpeg and `adb` are bundled; `libva`,
  `libdrm`, Vulkan and PipeWire come from the system. On first run the tray offers *Enable input*,
  which installs the `/dev/uinput` udev rule through polkit (the packages install it themselves).
  Moving the AppImage breaks its autostart entry: switch *Start with the session* off and on
  again. For menu integration use Gear Lever or AppImageLauncher. Diagnostics:
  `./ezdisplay-*.AppImage host diag`.

The packages install `ezdisplay` (the tray app) and `ezd-host`, the udev rule, the desktop entry and
the icon, and pull in `adb`. Start *ezdisplay* from the application launcher: it lives in the
system tray, and *Start with the session* in its menu adds it to autostart. On GNOME the tray needs
the AppIndicator extension. How the packages are built:
[packaging/linux/README.md](packaging/linux/README.md).

**Linux, from source.** Build it (below), then:

1. For touch and pen input, install the udev rule for `/dev/uinput` once (see
   [docs/input.md](docs/input.md)):

   ```sh
   sudo install -m644 packaging/linux/70-ezdisplay-uinput.rules /etc/udev/rules.d/
   sudo udevadm control --reload && sudo udevadm trigger --name-match=uinput
   ```

   For USB without developer mode, install `packaging/linux/70-ezdisplay-usb.rules` the same way and
   run `sudo udevadm trigger --subsystem-match=usb`.

2. Install the app and its desktop entry:

   ```sh
   install -Dm755 target/release/ezdisplay ~/.local/bin/ezdisplay
   install -Dm644 packaging/linux/com.librestatic.ezdisplay.desktop -t ~/.local/share/applications/
   install -Dm644 packaging/linux/com.librestatic.ezdisplay.svg -t ~/.local/share/icons/hicolor/scalable/apps/
   ```

3. Start *ezdisplay* from the application launcher.

`adb` comes from the Android SDK platform-tools (`adb` on the `PATH`).

## Building

Requirements: Rust (the pinned toolchain installs itself through `rustup`), FFmpeg 7 or 8
development libraries discoverable with `pkg-config` (or `FFMPEG_DIR` on Windows), and
`clang` for the bindings.

```sh
cargo build --workspace --release
cargo test --workspace
```

The tray app (`ezd-tray`) needs Rust 1.95 or later; the other crates build with 1.85.

Cross-checks of the platform-independent crates:

```sh
cargo check --target x86_64-pc-windows-msvc --no-default-features \
  -p ezd-proto -p ezd-link -p ezd-host-core -p ezd-host-windows -p ezd-host-service -p ezd-host -p ezd-tray
cargo check --target aarch64-linux-android -p ezd-proto -p ezd-link
```

## Trying it without a tablet (fake loopback)

Terminal 1, the host streaming a synthetic test pattern with a moving cursor:

```sh
cargo run --release -p ezd-host -- run --source synthetic
```

Terminal 2, the fake client; it decodes 300 frames, writes the 100th as a PPM image and prints
latency statistics (host capture to decoded picture):

```sh
cargo run --release -p ezd-fake-client -- --connect 127.0.0.1:27183 --frames 300 \
  --dump-frame 100 /path/to/frame100.ppm
```

With a real device, `ezd-host run --source synthetic --adb` sets up
`adb reverse tcp:27183 tcp:27183` for the attached device(s) and removes it on exit.

## Wi-Fi

`--wifi` also listens for tablets on the local network (QUIC over UDP port 27183, encrypted,
paired devices only). The host advertises itself over mDNS so tablets can find it:

```sh
ezd-host run --wifi
```

The first time, or with `--pair`, the host prints a QR code and a 6-digit PIN that are valid for
5 minutes. Once paired, a device reconnects without a PIN. `ezd-host paired list` shows the
paired devices and `ezd-host paired forget <name>` removes one. (The Android app's Wi-Fi
connect screen is in progress.)

The fake client pairs the same way, then streams:

```sh
cargo run --release -p ezd-fake-client -- --wifi 127.0.0.1:27183 --pin 123456 --frames 300
```

### Firewall (Linux)

A host firewall such as ufw or firewalld usually blocks inbound UDP 27183, so tablets cannot
connect. `ezd-host diag` reports the state (`firewall` key) and `ezd-host run --wifi` prints a hint
when one is active. The Arch package and the `.deb` install a ufw application profile and a firewalld
service named `ezdisplay` but never open the port: that is your decision. Allow it only for your LAN.

ufw (replace the network with yours):

```sh
sudo ufw allow from 192.168.0.0/24 to any app ezdisplay
# without the profile (AppImage, source builds):
sudo ufw allow from <your LAN>/24 to any port 27183 proto udp
```

firewalld:

```sh
sudo firewall-cmd --permanent --add-service=ezdisplay && sudo firewall-cmd --reload
```

The service is added to the default zone; to limit it to your home network, add `--zone=home` (or
`trusted`) to both commands, and make sure your Wi-Fi interface is in that zone. Without the
packaged service, use `--add-port=27183/udp` instead. The AppImage cannot install the profiles; copy
them from `packaging/linux/` (see its README) or use the port forms above.

### Firewall (Windows)

The Windows installer already adds an inbound UDP 27183 rule for private networks. For a portable
build, allow that port in Windows Defender Firewall for private networks.

Wi-Fi also works without pairing through ADB wireless debugging:
`ezd-host run --adb <ip:port> --link network`.

How the link and the pairing work: [docs/protocol.md](docs/protocol.md#transports).

## Diagnostics

```sh
cargo run --release -p ezd-host -- diag | python3 -m json.tool
```

prints OS, desktop session, GPUs, which encoders actually work on this machine (software,
NVENC, VAAPI per render node, QSV, AMF) and attached adb devices. Failures are reported as data.

## License

Dual licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
