//! The `run` subcommand.

use crate::i18n::fl;
use anyhow::{Result, anyhow};
use clap::{Args, Command, ValueEnum};
use ezd_host_core::SessionSummary;
use ezd_host_service::{
    AdbMode, HostEvent, HostService, HostStatus, LinkChoice, PenChoice, ServiceConfig,
    ServiceError, SourceKind,
};
use i18n_embed::fluent::FluentLanguageLoader;
use std::net::ToSocketAddrs;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Source {
    Synthetic,
    Virtual,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LinkArg {
    Usb,
    Network,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum PenModeArg {
    Tablet,
    Mouse,
}

// Option help texts live in i18n/<lang>/ezd-host.ftl (`run-<option>` ids); see `localize`.
#[derive(Args, Debug)]
pub struct RunArgs {
    #[arg(long, value_enum)]
    source: Source,
    #[arg(long, default_value = "127.0.0.1:27183")]
    listen: String,
    #[arg(long, num_args = 0..=1, value_name = "SERIAL")]
    adb: Option<Option<String>>,
    #[arg(long, value_enum, default_value = "usb")]
    link: LinkArg,
    #[arg(long, default_value_t = 120_000)]
    max_refresh_mhz: u32,
    #[arg(long)]
    bitrate_kbps: Option<u32>,
    #[arg(long)]
    encoder: Option<String>,
    #[arg(long)]
    adapter: Option<String>,
    #[arg(long)]
    prefer_dgpu: bool,
    #[arg(long)]
    output: Option<String>,
    #[arg(long, default_value_t = 1.0, value_parser = parse_scale)]
    scale: f32,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=8))]
    video_window: u32,
    #[arg(long)]
    telemetry: bool,
    #[arg(long)]
    no_input: bool,
    #[arg(long, value_enum, default_value = "tablet")]
    pen_mode: PenModeArg,
}

/// Applies the localized help of every `run` option.
pub fn localize(cmd: Command, l: &FluentLanguageLoader) -> Command {
    use i18n_embed_fl::fl;
    cmd.mut_arg("source", |a| a.help(fl!(l, "run-source")))
        .mut_arg("listen", |a| a.help(fl!(l, "run-listen")))
        .mut_arg("adb", |a| a.help(fl!(l, "run-adb")))
        .mut_arg("link", |a| a.help(fl!(l, "run-link")))
        .mut_arg("max_refresh_mhz", |a| a.help(fl!(l, "run-max-refresh-mhz")))
        .mut_arg("bitrate_kbps", |a| a.help(fl!(l, "run-bitrate-kbps")))
        .mut_arg("encoder", |a| a.help(fl!(l, "run-encoder")))
        .mut_arg("adapter", |a| a.help(fl!(l, "run-adapter")))
        .mut_arg("prefer_dgpu", |a| a.help(fl!(l, "run-prefer-dgpu")))
        .mut_arg("output", |a| a.help(fl!(l, "run-output")))
        .mut_arg("scale", |a| a.help(fl!(l, "run-scale")))
        .mut_arg("video_window", |a| a.help(fl!(l, "run-video-window")))
        .mut_arg("telemetry", |a| a.help(fl!(l, "run-telemetry")))
        .mut_arg("no_input", |a| a.help(fl!(l, "run-no-input")))
        .mut_arg("pen_mode", |a| a.help(fl!(l, "run-pen-mode")))
}

fn parse_scale(s: &str) -> Result<f32, String> {
    let v: f32 = s.parse().map_err(|_| fl!("err-scale-nan", value = s))?;
    if (0.25..=1.0).contains(&v) {
        Ok(v)
    } else {
        Err(fl!("err-scale-range", value = s))
    }
}

/// The service configuration the command line describes.
fn config(args: &RunArgs) -> Result<ServiceConfig> {
    let listen = args
        .listen
        .to_socket_addrs()
        .and_then(|mut a| a.next().ok_or_else(|| std::io::Error::other("no address")))
        .map_err(|e| {
            anyhow!(fl!(
                "err-binding",
                addr = args.listen.as_str(),
                reason = e.to_string()
            ))
        })?;
    Ok(ServiceConfig {
        listen,
        source: match args.source {
            Source::Synthetic => SourceKind::Synthetic,
            Source::Virtual => SourceKind::Virtual,
        },
        // Without `--adb` the host leaves adb alone.
        adb: match &args.adb {
            None => AdbMode::Off,
            Some(None) => AdbMode::Auto,
            Some(Some(serial)) => AdbMode::Serial(serial.clone()),
        },
        link: match args.link {
            LinkArg::Usb => LinkChoice::Usb,
            LinkArg::Network => LinkChoice::Network,
        },
        max_refresh_mhz: args.max_refresh_mhz,
        bitrate_kbps: args.bitrate_kbps,
        encoder: args.encoder.clone(),
        scale_permille: (args.scale * 1000.0).round() as u16,
        adapter: args.adapter.clone(),
        prefer_dgpu: args.prefer_dgpu,
        output_name: args.output.clone(),
        video_window: args.video_window,
        telemetry: args.telemetry,
        input: !args.no_input,
        pen_mode: match args.pen_mode {
            PenModeArg::Tablet => PenChoice::Tablet,
            PenModeArg::Mouse => PenChoice::Mouse,
        },
        // The command line never starts the app: that is the tray's convenience.
        open_app_on_connect: false,
    })
}

/// User-facing text of a service error.
pub fn localize_error(e: &ServiceError) -> String {
    match e {
        ServiceError::Bind { addr, reason } => {
            fl!(
                "err-binding",
                addr = addr.to_string(),
                reason = reason.as_str()
            )
        }
        ServiceError::AdbNotFound => fl!("err-adb-not-found"),
        ServiceError::AdbReverse { serial, reason } => {
            fl!(
                "err-adb-reverse",
                serial = serial.as_str(),
                reason = reason.as_str()
            )
        }
        ServiceError::UnsupportedPlatform => fl!("err-no-backend"),
        ServiceError::Source(reason) => fl!("err-source", reason = reason.as_str()),
    }
}

pub async fn run(args: RunArgs) -> Result<()> {
    let service = HostService::start(&tokio::runtime::Handle::current(), config(&args)?)
        .map_err(|e| anyhow!(localize_error(&e)))?;
    let mut events = service.events();
    let reporter = tokio::spawn(report_errors(service.status()));

    // The service owns the event sender, so the channel stays open until `stop`.
    loop {
        tokio::select! {
            ev = events.recv() => log_event(ev),
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    // Let the session stop the source: exiting while it tears the encoder down can deadlock in
    // the driver.
    tracing::info!("interrupted, shutting down");
    let stop = service.stop();
    tokio::pin!(stop);
    loop {
        tokio::select! {
            () = &mut stop => break,
            ev = events.recv() => log_event(ev),
            _ = tokio::signal::ctrl_c() => {
                tracing::warn!("interrupted again, exiting now");
                return Ok(());
            }
        }
    }
    // Whatever the session reported while stopping.
    while let Ok(ev) = events.try_recv() {
        log_event(Ok(ev));
    }
    reporter.abort();
    Ok(())
}

fn log_event(ev: Result<HostEvent, tokio::sync::broadcast::error::RecvError>) {
    match ev {
        Ok(HostEvent::SessionEnded(r)) => log_session_end(r),
        Ok(_) => {}
        Err(e) => tracing::debug!("missed service events: {e}"),
    }
}

/// Tells the user, in their language, about problems that need attention (adb missing, ...).
async fn report_errors(mut status: tokio::sync::watch::Receiver<HostStatus>) {
    let mut last = None;
    while status.changed().await.is_ok() {
        let err = match &*status.borrow_and_update() {
            HostStatus::Error(e) => Some(e.clone()),
            _ => None,
        };
        if err != last {
            if let Some(e) = &err {
                eprintln!("{}", localize_error(e));
            }
            last = err;
        }
    }
}

fn log_session_end(r: Result<SessionSummary, String>) {
    match r {
        Ok(s) => tracing::info!(
            end = ?s.end, frames = s.frames_sent, keyframes = s.keyframes_sent,
            input_events = s.input_events,
            seconds = s.duration.as_secs_f32(), "session finished"
        ),
        Err(e) => tracing::warn!("session failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_scale;

    #[test]
    fn scale_parsing() {
        assert_eq!(parse_scale("0.75"), Ok(0.75));
        assert_eq!(parse_scale("0.25"), Ok(0.25));
        assert_eq!(parse_scale("1"), Ok(1.0));
        for bad in ["0.24", "1.01", "0", "-1", "abc", "NaN", ""] {
            assert!(parse_scale(bad).is_err(), "{bad}");
        }
    }
}
