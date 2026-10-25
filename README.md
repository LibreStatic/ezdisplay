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

Not yet: AOA and Wi-Fi transports, audio, GNOME and wlroots backends.

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

**Linux (KDE Plasma).** Build from source (below), then:

1. For touch and pen input, install the udev rule for `/dev/uinput` once (see
   [docs/input.md](docs/input.md)):

   ```sh
   sudo install -m644 packaging/linux/70-ezdisplay-uinput.rules /etc/udev/rules.d/
   sudo udevadm control --reload && sudo udevadm trigger --name-match=uinput
   ```

2. Install the app and its desktop entry:

   ```sh
   install -Dm755 target/release/ezdisplay ~/.local/bin/ezdisplay
   install -Dm644 packaging/linux/com.librestatic.ezdisplay.desktop -t ~/.local/share/applications/
   install -Dm644 packaging/linux/com.librestatic.ezdisplay.svg -t ~/.local/share/icons/hicolor/scalable/apps/
   ```

3. Start *ezdisplay* from the application launcher. It lives in the system tray; *Start with the
   session* in its menu adds it to autostart. On GNOME the tray needs the AppIndicator extension.

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

## Diagnostics

```sh
cargo run --release -p ezd-host -- diag | python3 -m json.tool
```

prints OS, desktop session, GPUs, which encoders actually work on this machine (software,
NVENC, VAAPI per render node, QSV, AMF) and attached adb devices. Failures are reported as data.

## License

Dual licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
