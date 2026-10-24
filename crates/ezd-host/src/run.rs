//! The `run` subcommand.

use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};
use ezd_host_core::{
    SessionError, SessionOptions, SessionSummary, VideoSource, run_host_session_with,
};
use ezd_link::adb::{Adb, DeviceState};
use ezd_link::{Link, VideoCredit};
use ezd_proto::{DEFAULT_PORT, LinkKind, Prefs};
use tokio::net::TcpListener;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Source {
    /// Software-generated test pattern with a moving cursor.
    Synthetic,
    /// A real virtual display captured and encoded by the platform backend.
    Virtual,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LinkArg {
    Usb,
    Network,
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Video source.
    #[arg(long, value_enum)]
    source: Source,
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:27183")]
    listen: String,
    /// Run `adb reverse tcp:27183 tcp:<listen port>` for SERIAL, or for every
    /// ready device when no serial is given; removed again on exit.
    #[arg(long, num_args = 0..=1, value_name = "SERIAL")]
    adb: Option<Option<String>>,
    /// Link kind: selects the default bitrate ceiling (usb 80 Mbps, network 30 Mbps).
    #[arg(long, value_enum, default_value = "usb")]
    link: LinkArg,
    /// Highest refresh rate to use, in millihertz.
    #[arg(long, default_value_t = 120_000)]
    max_refresh_mhz: u32,
    /// Fixed bitrate in kbit/s (clamped to the link ceiling).
    #[arg(long)]
    bitrate_kbps: Option<u32>,
    /// Force a specific encoder by name.
    #[arg(long)]
    encoder: Option<String>,
    /// GPU/adapter to use (`--source virtual`).
    #[arg(long)]
    adapter: Option<String>,
    /// Prefer the discrete GPU (`--source virtual`).
    #[arg(long)]
    prefer_dgpu: bool,
    /// Capture this existing output instead of creating one (`--source virtual`).
    #[arg(long)]
    output: Option<String>,
    /// Stream at this fraction of the tablet's native resolution (0.25-1.0). Lower values
    /// reduce GPU load; the tablet scales the picture up.
    #[arg(long, default_value_t = 1.0, value_parser = parse_scale)]
    scale: f32,
    /// Video frames that may be sent but not yet acknowledged by the tablet. 1 gives the
    /// lowest latency; more raises the frame rate when the round trip is long (Wi-Fi).
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=8))]
    video_window: u32,
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
        bail!("no ready adb device found (run `ezd-host diag` to inspect)");
    }
    let mut guard = ReverseGuard {
        adb,
        serials: Vec::new(),
    };
    for s in serials {
        guard
            .adb
            .reverse(&s, DEFAULT_PORT, host_port)
            .with_context(|| format!("adb reverse for {s}"))?;
        tracing::info!(serial = %s, "adb reverse tcp:{DEFAULT_PORT} -> tcp:{host_port}");
        guard.serials.push(s);
    }
    Ok(guard)
}

fn parse_scale(s: &str) -> Result<f32, String> {
    let v: f32 = s.parse().map_err(|_| format!("`{s}` is not a number"))?;
    if (0.25..=1.0).contains(&v) {
        Ok(v)
    } else {
        Err(format!("scale must be between 0.25 and 1.0, got {s}"))
    }
}

pub async fn run(args: RunArgs) -> Result<()> {
    let listener = TcpListener::bind(&args.listen)
        .await
        .with_context(|| format!("binding {}", args.listen))?;
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

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let session_opts = SessionOptions {
        video_credit: VideoCredit::OnAck {
            window: args.video_window,
            timeout: std::time::Duration::from_millis(500),
        },
        shutdown: Some(shutdown_rx),
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
