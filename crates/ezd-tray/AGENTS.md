# ezd-tray (binary `ezdisplay`)

The PC tray app: owns the `ezd-host-service` `HostService`, shows its state in the system tray and opens a
settings and statistics window (egui/eframe, glow renderer) on demand. Rust 1.95 (eframe 0.37); the rest of
the workspace stays on 1.85. Run cargo with the rustup toolchain (`PATH=$HOME/.cargo/bin:$PATH cargo ...`).

## Layout
- `main.rs`: arguments, single-instance claim, runtime, main loop. `cli.rs`: localized clap command
  (`--settings`, `--quit`, `--autostart on|off`, hidden `--config <path>`).
- `controller.rs`: `Controller` (main thread): config, service handle, tray, event queue. `tick()` drains
  tray/menu events, IPC requests, service stop notifications and status changes.
- `state.rs`: pure `Lifecycle` (Stopped, Running, Stopping{restart}); a Start requested while Stopping is
  queued and runs after the old service finished (it still holds the listen port).
- `menu_model.rs`: the tray menu as a pure function of state (unit tested); `tray.rs`: applies it to the
  native items (only changed items are touched; each setter republishes the menu over D-Bus on Linux).
- `ui/view.rs`: the window as a plain view over a `Snapshot`; it records `Action`s and never touches the
  tray or the service. `ui/app.rs`: eframe glue (`logic` calls `Controller::tick`). `ui/theme.rs`: palettes.
- `config.rs` (`config.toml`), `ipc.rs` (local socket), `autostart.rs`, `logging.rs`, `icons.rs`,
  `platform.rs`, `text.rs` (status wording), `i18n.rs` + `i18n/<lang>/ezd-tray.ftl`.
- All unsafe Win32 code lives in `ezd-host-windows` (`ensure_dpi_aware`, `pump_messages`, `run_elevated`).

## Threads
Main thread: tray (Windows needs its message loop: `pump_messages`), `Controller`, the settings window
(`eframe::run_native` returns when it closes; the idle loop resumes). The tokio runtime runs the service.
Linux: the ksni backend, the IPC accept thread and a forwarder thread feed the controller's channel.
Quit stops the service and waits for it (30 s cap) before exiting: hardware encoders can deadlock if the
process exits mid-teardown.

## Running with a test config
`--config <file>` also gives that run its own instance name, so it never talks to the real app. Use a file
with `start_on_launch = false` and `[service] adb = "off"` (or `{ serial = "<tablet>" }`):

    ezdisplay --config /tmp/ezd-test.toml &
    ezdisplay --config /tmp/ezd-test.toml --quit

The tray needs a StatusNotifier host. GNOME needs the AppIndicator extension to show it.

## Rendering the window
1. Semantic tests (CI, no display): `cargo test -p ezd-tray` drives `SettingsView` through egui_kittest and
   reads the AccessKit tree (texts in en and es, config edits, actions).
2. PNG snapshots (local, needs a GPU): `cargo test -p ezd-tray --features snapshots`, refresh with
   `UPDATE_SNAPSHOTS=1`. They land in `crates/ezd-tray/tests/snapshots/` and are not committed.

## Icons
`assets/gen-icons.sh` regenerates the status PNGs, `ezdisplay.ico` and the Linux launcher SVG from
`assets/icons/*.svg` (rsvg-convert and magick).
