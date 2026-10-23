//! `ezd-fake-client`: connect to an `ezd-host`, decode the stream, print latency statistics.

use anyhow::{Context, Result};
use clap::Parser;
use ezd_fake_client::{FakeClientConfig, run_fake_client};
use ezd_link::Link;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Desktop stand-in for the Android client.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Host address (e.g. the `adb reverse` endpoint on a device is `127.0.0.1:27183`).
    #[arg(long, default_value = "127.0.0.1:27183")]
    connect: String,
    /// Fake display width.
    #[arg(long, default_value_t = 1920)]
    width: u32,
    /// Fake display height.
    #[arg(long, default_value_t = 1080)]
    height: u32,
    /// Fake display refresh rates in millihertz, comma separated.
    #[arg(long, value_delimiter = ',', default_values_t = [60000u32, 120000])]
    refresh_rates_mhz: Vec<u32>,
    /// Exit after this many decoded frames.
    #[arg(long)]
    frames: Option<u64>,
    /// Write the Nth decoded frame (1-based) to a PPM file: `--dump-frame 100 out.ppm`.
    #[arg(long, num_args = 2, value_names = ["N", "PATH"])]
    dump_frame: Option<Vec<String>>,
    /// Device name sent in Hello.
    #[arg(long, default_value = "ezd-fake-client")]
    name: String,
    /// Do not interpret frames as the synthetic test pattern.
    #[arg(long)]
    no_verify: bool,
    /// Keep retrying the TCP connection for this many seconds.
    #[arg(long, default_value_t = 10)]
    connect_timeout: u64,
    /// Abort after this many seconds.
    #[arg(long)]
    max_seconds: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let a = Args::parse();
    let dump_frame = match a.dump_frame {
        Some(v) => {
            let n: u64 = v[0].parse().context("--dump-frame N must be a number")?;
            Some((n, PathBuf::from(&v[1])))
        }
        None => None,
    };
    let cfg = FakeClientConfig {
        device_name: a.name,
        width: a.width,
        height: a.height,
        refresh_rates_mhz: a.refresh_rates_mhz,
        frames: a.frames,
        dump_frame,
        verify_synthetic: !a.no_verify,
        max_duration: a.max_seconds.map(Duration::from_secs),
        ..Default::default()
    };

    let start = Instant::now();
    let link = loop {
        match Link::connect_tcp(&a.connect).await {
            Ok(l) => break l,
            Err(e) if start.elapsed() < Duration::from_secs(a.connect_timeout) => {
                tracing::debug!("connect failed ({e}), retrying");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(e) => return Err(e).with_context(|| format!("connecting to {}", a.connect)),
        }
    };
    tracing::info!(host = %a.connect, "connected");
    let report = run_fake_client(link, cfg).await?;
    println!("{}", report.summary());
    Ok(())
}
