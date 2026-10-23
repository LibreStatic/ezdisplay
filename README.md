# ezdisplay

Use an Android tablet as an extended monitor for your PC over USB, with very low latency.

The host (Linux and Windows, Rust) renders a virtual display and encodes it on the same GPU
without copies; the tablet (Kotlin UI + Rust native library) decodes and shows it. The mouse
cursor is sent separately and drawn locally on the tablet, so pointer movement never waits
for a video frame.

## Status

Phase 1a, the shared core, is done: wire protocol, transport (TCP / ADB reverse), prioritized
mux, host session logic, FFmpeg wrapper, a software test source and a desktop fake client.
Not yet: the Android client (1b), the Linux/KDE host backend (1c), the Windows host backend
(1d), input injection (phase 2), AOA and Wi-Fi transports.

| Crate | Purpose |
| --- | --- |
| `ezd-proto` | Wire framing, messages, capability negotiation, clock sync. Pure, builds for Android. |
| `ezd-link` | `Link` byte streams (TCP, in-memory), ADB helper, prioritized mux. |
| `ezd-host-core` | `VideoSource` / `VirtualDisplay` traits, host session, synthetic test frames. |
| `ezd-ffmpeg` | Safe FFmpeg wrapper: low-latency `Encoder`, `Decoder`, `SwVideoSource`, encoder probing. |
| `ezd-fake-client` | Desktop stand-in for the Android client (decodes, measures latency). |
| `ezd-host` | The host binary: `run` and `diag`. |

The protocol is specified in [docs/protocol.md](docs/protocol.md).

## Building

Requirements: Rust (the pinned toolchain installs itself through `rustup`), FFmpeg 7 or 8
development libraries discoverable with `pkg-config` (or `FFMPEG_DIR` on Windows), and
`clang` for the bindings.

```sh
cargo build --workspace --release
cargo test --workspace
```

Cross-checks of the platform-independent crates:

```sh
cargo check --target x86_64-pc-windows-msvc -p ezd-proto -p ezd-link -p ezd-host-core
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
