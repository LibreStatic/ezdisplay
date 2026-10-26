//! The `run` subcommand.

use crate::i18n::fl;
use anyhow::{Result, anyhow};
use clap::{Args, Command, ValueEnum};
use ezd_host_core::SessionSummary;
use ezd_host_service::{
    AdbMode, AoaMode, HostEvent, HostService, HostStatus, LinkChoice, PenChoice, ServiceConfig,
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
pub enum AoaArg {
    Auto,
    Off,
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
    #[arg(long, value_enum, default_value = "auto")]
    aoa: AoaArg,
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
    #[cfg(feature = "wifi")]
    #[arg(long, num_args = 0..=1, value_name = "ADDR", default_missing_value = "0.0.0.0:27183")]
    wifi: Option<String>,
    #[cfg(feature = "wifi")]
    #[arg(long, requires = "wifi")]
    pair: bool,
    #[cfg(feature = "wifi")]
    #[arg(long, value_enum, hide = true, requires = "wifi")]
    congestion: Option<crate::wifi::CongestionArg>,
    #[cfg(feature = "wifi")]
    #[arg(long, hide = true, requires = "wifi", value_name = "DIR")]
    config_dir: Option<std::path::PathBuf>,
}

/// Applies the localized help of every `run` option.
pub fn localize(cmd: Command, l: &FluentLanguageLoader) -> Command {
    use i18n_embed_fl::fl;
    let cmd = cmd
        .mut_arg("source", |a| a.help(fl!(l, "run-source")))
        .mut_arg("listen", |a| a.help(fl!(l, "run-listen")))
        .mut_arg("adb", |a| a.help(fl!(l, "run-adb")))
        .mut_arg("aoa", |a| a.help(fl!(l, "run-aoa")))
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
        .mut_arg("pen_mode", |a| a.help(fl!(l, "run-pen-mode")));
    #[cfg(feature = "wifi")]
    let cmd = cmd
        .mut_arg("wifi", |a| a.help(fl!(l, "run-wifi")))
        .mut_arg("pair", |a| a.help(fl!(l, "run-pair")));
    cmd
}

fn parse_scale(s: &str) -> Result<f32, String> {
    let v: f32 = s.parse().map_err(|_| fl!("err-scale-nan", value = s))?;
    if (0.25..=1.0).contains(&v) {
        Ok(v)
    } else {
        Err(fl!("err-scale-range", value = s))
    }
}

/// The first socket address `addr` resolves to.
fn resolve(addr: &str) -> Result<std::net::SocketAddr> {
    addr.to_socket_addrs()
        .and_then(|mut a| a.next().ok_or_else(|| std::io::Error::other("no address")))
        .map_err(|e| anyhow!(fl!("err-binding", addr = addr, reason = e.to_string())))
}

/// The service configuration the command line describes.
fn config(args: &RunArgs) -> Result<ServiceConfig> {
    let listen = resolve(&args.listen)?;
    #[cfg(feature = "wifi")]
    let wifi = match &args.wifi {
        None => None,
        Some(addr) => Some(ezd_host_service::WifiConfig {
            listen: resolve(addr)?,
            pair_on_start: args.pair,
            congestion: args.congestion.map(Into::into).unwrap_or_default(),
            config_dir: args.config_dir.clone(),
        }),
    };
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
        aoa: match args.aoa {
            AoaArg::Auto => AoaMode::Auto,
            AoaArg::Off => AoaMode::Off,
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
        #[cfg(feature = "wifi")]
        wifi,
        // The command line has no HDR switch: it streams HDR10 whenever everything supports it.
        ..ServiceConfig::default()
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
        ServiceError::Wifi(reason) => fl!("err-wifi", reason = reason.as_str()),
        ServiceError::Source(reason) => fl!("err-source", reason = reason.as_str()),
    }
}

pub async fn run(args: RunArgs) -> Result<()> {
    let service = HostService::start(&tokio::runtime::Handle::current(), config(&args)?)
        .map_err(|e| anyhow!(localize_error(&e)))?;
    let mut events = service.events();
    #[cfg(feature = "wifi")]
    {
        if let Some(info) = service.wifi().borrow().clone() {
            crate::wifi::print_listening(info.addr, &info.fingerprint_short);
        }
        // The window may have opened before the subscription above.
        service.refresh_pairing();
    }
    let mut shown: Shown = None;
    let reporter = tokio::spawn(report_errors(service.status()));
    let aoa_notice = tokio::spawn(report_aoa_problem(service.aoa_problem()));

    // The service owns the event sender, so the channel stays open until `stop`.
    loop {
        tokio::select! {
            ev = events.recv() => log_event(ev, &mut shown),
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
            ev = events.recv() => log_event(ev, &mut shown),
            _ = tokio::signal::ctrl_c() => {
                tracing::warn!("interrupted again, exiting now");
                return Ok(());
            }
        }
    }
    // Whatever the session reported while stopping.
    while let Ok(ev) = events.try_recv() {
        log_event(Ok(ev), &mut shown);
    }
    reporter.abort();
    aoa_notice.abort();
    Ok(())
}

/// The pairing window last shown, to not print it twice.
type Shown = Option<(String, std::time::Instant)>;

#[cfg_attr(not(feature = "wifi"), allow(unused_variables))]
fn log_event(ev: Result<HostEvent, tokio::sync::broadcast::error::RecvError>, shown: &mut Shown) {
    match ev {
        Ok(HostEvent::SessionEnded(r)) => log_session_end(r),
        #[cfg(feature = "wifi")]
        Ok(HostEvent::PairingOpen {
            pin,
            uri,
            fingerprint,
            expires,
        }) => {
            // The service may announce the same window twice (late subscription).
            if shown.replace((pin.clone(), expires)) != Some((pin.clone(), expires)) {
                crate::wifi::print_pairing_open(&pin, &uri, &fingerprint, expires);
            }
        }
        #[cfg(feature = "wifi")]
        Ok(HostEvent::PairingClosed { reason }) => {
            *shown = None;
            crate::wifi::print_pairing_closed(reason);
        }
        #[cfg(feature = "wifi")]
        Ok(HostEvent::Paired { device }) => crate::wifi::print_paired(&device),
        #[cfg(feature = "wifi")]
        Ok(HostEvent::PairingFailed { peer, reason }) => {
            tracing::warn!(%peer, "pairing failed: {reason}")
        }
        #[cfg(feature = "wifi")]
        Ok(HostEvent::SessionRefused { peer, why }) => {
            tracing::info!(%peer, ?why, "Wi-Fi session refused")
        }
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

/// Prints a one-line notice whenever USB accessory mode turns out to be unavailable. It is not an
/// error: adb and the network keep working.
async fn report_aoa_problem(mut problem: tokio::sync::watch::Receiver<Option<String>>) {
    loop {
        let reason = problem.borrow_and_update().clone();
        if let Some(reason) = reason {
            eprintln!("{}", fl!("aoa-unavailable", reason = reason.as_str()));
        }
        if problem.changed().await.is_err() {
            break;
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
