//! The `run` subcommand.

use crate::i18n::fl;
use anyhow::{Context, Result, bail};
use clap::{Args, Command, ValueEnum};
use ezd_host_core::{
    InputBackend, PenMode, SessionError, SessionOptions, SessionSummary, UnavailableBackend,
    VideoSource, run_host_session_with,
};
use ezd_link::adb::{Adb, DeviceState};
use ezd_link::{Link, VideoCredit};
use ezd_proto::{DEFAULT_PORT, LinkKind, Prefs};
use i18n_embed::fluent::FluentLanguageLoader;
use std::sync::Arc;
use tokio::net::TcpListener;

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

/// Removes the adb reverse forwards when dropped.
struct ReverseGuard {
    adb: Adb,
    serials: Vec<String>,
}

impl Drop for ReverseGuard {
    fn drop(&mut self) {
        for s in &self.serials {
            match self.adb.remove_reverse(s, DEFAULT_PORT) {
                Ok(()) => tracing::info!(serial = %s, "removed adb reverse"),
                Err(e) => tracing::warn!(serial = %s, "could not remove adb reverse: {e}"),
            }
        }
    }
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

fn setup_adb(serial: Option<&str>, host_port: u16) -> Result<ReverseGuard> {
    let adb = Adb::locate()?;
    let serials: Vec<String> = match serial {
        Some(s) => vec![s.to_string()],
        None => adb
            .devices()?
            .into_iter()
            .filter(|d| d.state == DeviceState::Device)
            .map(|d| d.serial)
            .collect(),
    };
    if serials.is_empty() {
        bail!(fl!("err-no-adb-device"));
    }
    let mut guard = ReverseGuard {
        adb,
        serials: Vec::new(),
    };
    for s in serials {
        guard
            .adb
            .reverse(&s, DEFAULT_PORT, host_port)
            .with_context(|| fl!("err-adb-reverse", serial = s.as_str()))?;
        tracing::info!(serial = %s, "adb reverse tcp:{DEFAULT_PORT} -> tcp:{host_port}");
        guard.serials.push(s);
    }
    Ok(guard)
}

fn parse_scale(s: &str) -> Result<f32, String> {
    let v: f32 = s.parse().map_err(|_| fl!("err-scale-nan", value = s))?;
    if (0.25..=1.0).contains(&v) {
        Ok(v)
    } else {
        Err(fl!("err-scale-range", value = s))
    }
}

pub async fn run(args: RunArgs) -> Result<()> {
    let listener = TcpListener::bind(&args.listen)
        .await
        .with_context(|| fl!("err-binding", addr = args.listen.as_str()))?;
    let local = listener.local_addr()?;
    tracing::info!("listening on {local}");
    let _reverse = match &args.adb {
        Some(serial) => Some(setup_adb(serial.as_deref(), local.port())?),
        None => None,
    };
    let prefs = Prefs {
        max_refresh_mhz: args.max_refresh_mhz,
        link: match args.link {
            LinkArg::Usb => LinkKind::Usb,
            LinkArg::Network => LinkKind::Network,
        },
        bitrate_kbps: args.bitrate_kbps,
        encoder: args.encoder.clone(),
        scale_permille: (args.scale * 1000.0).round() as u16,
        ..Prefs::default()
    };

    let platform_opts = ezd_host_core::PlatformOptions {
        adapter: args.adapter.clone(),
        prefer_dgpu: args.prefer_dgpu,
        output_name: args.output.clone(),
    };

    let input: Option<Arc<dyn InputBackend>> = if args.no_input {
        None
    } else {
        Some(crate::platform::input_backend().unwrap_or_else(|| {
            Arc::new(UnavailableBackend(
                "input injection is not implemented on this platform yet".into(),
            ))
        }))
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let session_opts = SessionOptions {
        video_credit: VideoCredit::OnAck {
            window: args.video_window,
            timeout: std::time::Duration::from_millis(500),
        },
        shutdown: Some(shutdown_rx),
        input,
        pen_mode: match args.pen_mode {
            PenModeArg::Tablet => PenMode::Tablet,
            PenModeArg::Mouse => PenMode::Mouse,
        },
        telemetry: args.telemetry,
        ..SessionOptions::default()
    };

    loop {
        let (sock, peer) = tokio::select! {
            r = listener.accept() => r?,
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("interrupted, shutting down");
                return Ok(());
            }
        };
        tracing::info!(%peer, "client connected");
        let link = Link::from_tcp(sock)?;
        let source: Box<dyn VideoSource> = match args.source {
            Source::Synthetic => Box::new(ezd_ffmpeg::SwVideoSource::new()),
            Source::Virtual => match crate::platform::create_virtual_source(&platform_opts) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("cannot create virtual source: {e}");
                    continue;
                }
            },
        };
        let session = run_host_session_with(link, source, prefs.clone(), session_opts.clone());
        tokio::pin!(session);
        tokio::select! {
            r = &mut session => log_session_end(r),
            _ = tokio::signal::ctrl_c() => {
                // Let the session stop the source: exiting while it tears the encoder down can
                // deadlock in the driver.
                tracing::info!("interrupted, ending the session");
                let _ = shutdown_tx.send(true);
                tokio::select! {
                    r = &mut session => log_session_end(r),
                    _ = tokio::signal::ctrl_c() => tracing::warn!("interrupted again, exiting now"),
                }
                return Ok(());
            }
        }
    }
}

fn log_session_end(r: Result<SessionSummary, SessionError>) {
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
