//! `ezdisplay`: the tray app of the PC. It owns the host service, shows its state in the system
//! tray and opens a settings and statistics window on demand.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod autostart;
mod cli;
mod config;
mod controller;
mod i18n;
mod icons;
mod ipc;
mod logging;
mod menu_model;
mod paths;
mod platform;
mod state;
mod text;
mod tray;
mod ui;
#[cfg(target_os = "linux")]
mod uinput;

use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    #[cfg(windows)]
    {
        // Capture and input injection need real pixels, before any window exists.
        let _ = ezd_host_windows::ensure_dpi_aware();
        ezd_host_windows::isolate_std_handles();
    }
    let loader = i18n::desktop_loader();
    let args = cli::parse(&loader);

    if let Some(on) = args.autostart {
        return match autostart::set(on == cli::OnOff::On) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        };
    }

    let config_path = args
        .config
        .clone()
        .unwrap_or_else(paths::default_config_file);
    // Runs with another config file get their own instance name: they never talk to (or block)
    // the real app.
    let suffix = args.config.as_ref().map(|p| cli::path_tag(p));
    let socket = ipc::socket_name(suffix.as_deref());

    if args.quit {
        return cli::quit_running(&socket);
    }

    let _log_guard = logging::init(&paths::log_dir());
    tracing::info!("ezdisplay {} starting", env!("CARGO_PKG_VERSION"));

    // Claim the instance name before doing anything else.
    let server = match ipc::Server::bind(&socket, || {}) {
        Ok(s) => s,
        Err(ipc::BindError::InUse) => {
            tracing::info!("another instance is running; asking it to show itself");
            let _ = ipc::send(&socket, ipc::Command::Show, Duration::from_secs(5));
            return ExitCode::SUCCESS;
        }
        Err(ipc::BindError::Other(e)) => {
            tracing::warn!("cannot claim the instance name ({e}); continuing without it");
            return run(loader, args, config_path, None);
        }
    };
    run(loader, args, config_path, Some(server))
}

fn run(
    loader: i18n_embed::fluent::FluentLanguageLoader,
    args: cli::Args,
    config_path: std::path::PathBuf,
    server: Option<ipc::Server>,
) -> ExitCode {
    let loaded = config::load(&config_path);
    if let Some(e) = &loaded.error {
        tracing::error!("cannot read the settings, using defaults: {e}");
    }
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("ezd-rt")
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("cannot start the runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut controller = controller::Controller::new(
        rt.handle().clone(),
        loader,
        config_path,
        loaded.config,
        controller::Waker::default(),
    );
    let (tx, waker) = controller.sender();
    controller::forward_tray_events(tx.clone(), waker.clone());
    if let Some(server) = server {
        let _ = std::thread::Builder::new()
            .name("ipc-forward".into())
            .spawn(move || {
                while let Ok(req) = server.requests.recv() {
                    let _ = tx.send(controller::Event::Ipc(req));
                    waker.wake();
                }
            });
    }

    controller.create_tray();
    if controller.config().start_on_launch {
        controller.start();
    }
    if args.settings {
        controller.request_settings();
    }

    while !controller.should_exit() {
        #[cfg(windows)]
        {
            // The tray lives on this thread and needs its message loop.
            ezd_host_windows::pump_messages(Duration::from_millis(100));
            controller.tick();
        }
        #[cfg(not(windows))]
        controller.wait_and_tick(Duration::from_millis(100));

        if controller.take_open_request() && !controller.is_quitting() {
            if let Err(e) = ui::app::run_settings(&mut controller) {
                tracing::error!("settings window failed: {e}");
            }
            controller.tick();
        }
    }
    tracing::info!("exiting");
    rt.shutdown_timeout(Duration::from_secs(2));
    controller.finish();
    ExitCode::SUCCESS
}
