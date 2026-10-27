//! `ezd-fake-client`: connect to an `ezd-host`, decode the stream, print latency statistics.

use anyhow::{Context, Result};
use clap::{ArgMatches, Command, CommandFactory, FromArgMatches, Parser, ValueEnum};
use ezd_fake_client::fl;
use ezd_fake_client::i18n;
use ezd_fake_client::{FakeClientConfig, InputScript, run_fake_client};
use ezd_link::Link;
use ezd_proto::{ClientPrefs, Codec, QualityLevel};
use i18n_embed::fluent::FluentLanguageLoader;
use std::path::PathBuf;
use std::time::{Duration, Instant};
// Help texts live in i18n/<lang>/ezd-fake-client.ftl and are applied by `command`; the derive
// struct only describes the shape of the command line.
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:27183")]
    connect: String,
    #[arg(long, default_value_t = 1920)]
    width: u32,
    #[arg(long, default_value_t = 1080)]
    height: u32,
    #[arg(long, value_delimiter = ',', default_values_t = [60000u32, 120000])]
    refresh_rates_mhz: Vec<u32>,
    #[arg(long)]
    frames: Option<u64>,
    #[arg(long, num_args = 2, value_names = ["N", "PATH"])]
    dump_frame: Option<Vec<String>>,
    #[arg(long, default_value = "ezd-fake-client")]
    name: String,
    #[arg(long)]
    no_verify: bool,
    #[arg(long, default_value_t = 10)]
    connect_timeout: u64,
    #[arg(long)]
    max_seconds: Option<u64>,
    #[arg(long, value_enum)]
    input_script: Option<ScriptArg>,
    #[arg(long)]
    exit_after_script: bool,
    #[arg(long, value_enum, default_value_t = CodecArg::Auto)]
    codec: CodecArg,
    #[arg(long, value_enum, default_value_t = QualityArg::Normal)]
    quality: QualityArg,
    #[arg(long, hide = true)]
    proto_minor: Option<u16>,
    #[cfg(feature = "wifi")]
    #[arg(long, value_name = "ADDR", conflicts_with = "connect")]
    wifi: Option<String>,
    #[cfg(feature = "wifi")]
    #[arg(long, value_name = "PIN", conflicts_with = "uri")]
    pin: Option<String>,
    #[cfg(feature = "wifi")]
    #[arg(long, value_name = "URI", conflicts_with = "connect")]
    uri: Option<String>,
    #[cfg(feature = "wifi")]
    #[arg(long, value_name = "DIR")]
    state_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ScriptArg {
    Tap,
    Drag,
    Circle,
    Touch2,
    Scroll,
    Keys,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CodecArg {
    Auto,
    H264,
    Hevc,
    Av1,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum QualityArg {
    Normal,
    High,
    Lossless,
}

/// The command line with every help text taken from `l`.
fn command(l: &FluentLanguageLoader) -> Command {
    use i18n_embed_fl::fl;
    let cmd = Args::command();
    let cmd = cmd
        .about(fl!(l, "about"))
        .mut_arg("connect", |a| a.help(fl!(l, "arg-connect")))
        .mut_arg("width", |a| a.help(fl!(l, "arg-width")))
        .mut_arg("height", |a| a.help(fl!(l, "arg-height")))
        .mut_arg("refresh_rates_mhz", |a| {
            a.help(fl!(l, "arg-refresh-rates-mhz"))
        })
        .mut_arg("frames", |a| a.help(fl!(l, "arg-frames")))
        .mut_arg("dump_frame", |a| a.help(fl!(l, "arg-dump-frame")))
        .mut_arg("name", |a| a.help(fl!(l, "arg-name")))
        .mut_arg("no_verify", |a| a.help(fl!(l, "arg-no-verify")))
        .mut_arg("connect_timeout", |a| a.help(fl!(l, "arg-connect-timeout")))
        .mut_arg("max_seconds", |a| a.help(fl!(l, "arg-max-seconds")))
        .mut_arg("input_script", |a| a.help(fl!(l, "arg-input-script")))
        .mut_arg("exit_after_script", |a| {
            a.help(fl!(l, "arg-exit-after-script"))
        })
        .mut_arg("codec", |a| a.help(fl!(l, "arg-codec")))
        .mut_arg("quality", |a| a.help(fl!(l, "arg-quality")));
    #[cfg(feature = "wifi")]
    let cmd = cmd
        .mut_arg("wifi", |a| a.help(fl!(l, "arg-wifi")))
        .mut_arg("pin", |a| a.help(fl!(l, "arg-pin")))
        .mut_arg("uri", |a| a.help(fl!(l, "arg-uri")))
        .mut_arg("state_dir", |a| a.help(fl!(l, "arg-state-dir")));
    i18n::localize_version(i18n::localize_chrome(cmd, l), l)
}

fn parse(matches: &ArgMatches) -> Args {
    Args::from_arg_matches(matches).unwrap_or_else(|e| e.exit())
}

#[tokio::main]
async fn main() -> Result<()> {
    i18n::init();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let a = parse(&command(&i18n::LANGUAGE_LOADER).get_matches());
    #[cfg(feature = "wifi")]
    let wifi = wifi_options(&a)?;
    let dump_frame = match a.dump_frame {
        Some(v) => {
            let n: u64 = v[0].parse().with_context(|| fl!("err-dump-frame"))?;
            Some((n, PathBuf::from(&v[1])))
        }
        None => None,
    };
    let cfg = FakeClientConfig {
        device_name: a.name.clone(),
        width: a.width,
        height: a.height,
        refresh_rates_mhz: a.refresh_rates_mhz,
        frames: a.frames,
        dump_frame,
        verify_synthetic: !a.no_verify,
        max_duration: a.max_seconds.map(Duration::from_secs),
        input_script: a.input_script.map(|s| match s {
            ScriptArg::Tap => InputScript::Tap,
            ScriptArg::Drag => InputScript::Drag,
            ScriptArg::Circle => InputScript::Circle,
            ScriptArg::Touch2 => InputScript::Touch2,
            ScriptArg::Scroll => InputScript::Scroll,
            ScriptArg::Keys => InputScript::Keys,
        }),
        stop_after_script: a.exit_after_script,
        proto_minor: a.proto_minor,
        prefs: ClientPrefs::new(
            match a.codec {
                CodecArg::Auto => None,
                CodecArg::H264 => Some(Codec::H264),
                CodecArg::Hevc => Some(Codec::Hevc),
                CodecArg::Av1 => Some(Codec::Av1),
            },
            match a.quality {
                QualityArg::Normal => QualityLevel::Normal,
                QualityArg::High => QualityLevel::High,
                QualityArg::Lossless => QualityLevel::Lossless,
            },
        ),
        ..Default::default()
    };

    #[cfg(feature = "wifi")]
    if let Some(opts) = wifi {
        let (link, outcome) = ezd_fake_client::wifi::connect_wifi(&opts)
            .await
            .map_err(|e| wifi_error(&e))?;
        if let Some(host) = outcome.paired_with {
            println!(
                "{}",
                fl!(
                    "paired-with",
                    host = host.name.as_str(),
                    fingerprint = host.fingerprint.as_str()
                )
            );
        }
        tracing::info!("connected over Wi-Fi");
        let report = run_fake_client(link, cfg).await?;
        println!("{}", report.summary());
        return Ok(());
    }

    let start = Instant::now();
    let link = loop {
        match Link::connect_tcp(&a.connect).await {
            Ok(l) => break l,
            Err(e) if start.elapsed() < Duration::from_secs(a.connect_timeout) => {
                tracing::debug!("connect failed ({e}), retrying");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(e) => {
                return Err(e).with_context(|| fl!("err-connecting", addr = a.connect.as_str()));
            }
        }
    };
    tracing::info!(host = %a.connect, "connected");
    let report = run_fake_client(link, cfg).await?;
    println!("{}", report.summary());
    Ok(())
}

/// The Wi-Fi options the command line describes, if any.
#[cfg(feature = "wifi")]
fn wifi_options(a: &Args) -> Result<Option<ezd_fake_client::wifi::WifiOptions>> {
    use std::net::ToSocketAddrs;
    if a.wifi.is_none() && a.uri.is_none() {
        if a.pin.is_some() {
            anyhow::bail!(fl!("err-pin-needs-wifi"));
        }
        return Ok(None);
    }
    let uri = match &a.uri {
        Some(u) => Some(ezd_link::pair::PairUri::parse(u).with_context(|| fl!("err-uri-invalid"))?),
        None => None,
    };
    let addr = match &a.wifi {
        Some(addr) => Some(
            addr.to_socket_addrs()
                .ok()
                .and_then(|mut x| x.next())
                .with_context(|| fl!("err-connecting", addr = addr.as_str()))?,
        ),
        None => None,
    };
    let state_dir = a
        .state_dir
        .clone()
        .or_else(ezd_fake_client::wifi::default_state_dir)
        .with_context(|| fl!("err-no-state-dir"))?;
    Ok(Some(ezd_fake_client::wifi::WifiOptions {
        addr,
        pin: a.pin.clone(),
        uri,
        state_dir,
        device_name: a.name.clone(),
        connect_timeout: Duration::from_secs(a.connect_timeout),
    }))
}

/// User-facing text of a Wi-Fi failure.
#[cfg(feature = "wifi")]
fn wifi_error(e: &ezd_fake_client::wifi::WifiError) -> anyhow::Error {
    use ezd_fake_client::wifi::WifiError;
    use ezd_link::quic::QuicError;
    use ezd_proto::pair::Refusal;
    match e {
        WifiError::NoAddress => anyhow::anyhow!(fl!("err-wifi-no-address")),
        WifiError::NotPaired => anyhow::anyhow!(fl!("err-wifi-not-paired")),
        WifiError::Quic(QuicError::Refused(r)) => anyhow::anyhow!(match r {
            Refusal::NotPaired => fl!("err-refused-not-paired"),
            Refusal::PairingClosed => fl!("err-refused-closed"),
            Refusal::TooManyAttempts => fl!("err-refused-attempts"),
            Refusal::WrongPin => fl!("err-refused-wrong-pin"),
            Refusal::Busy => fl!("err-refused-busy"),
        }),
        WifiError::Quic(QuicError::FingerprintMismatch) => {
            anyhow::anyhow!(fl!("err-wifi-fingerprint"))
        }
        other => anyhow::anyhow!(fl!("err-wifi", reason = other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn help(lang: &str) -> String {
        let l = i18n::loader_for(&[lang.parse().unwrap()]);
        let err = command(&l)
            .try_get_matches_from(["ezd-fake-client", "--help"])
            .unwrap_err();
        err.render().to_string()
    }

    #[test]
    fn german_help_is_localized() {
        let h = help("de");
        assert!(h.contains("Desktop-Ersatz für den Android-Client."), "{h}");
        assert!(h.contains("Verwendung:") && h.contains("Optionen:"), "{h}");
        assert!(!h.contains("Usage:") && !h.contains("Print help"), "{h}");
    }

    #[test]
    fn english_is_the_fallback() {
        let h = help("en");
        assert!(
            h.contains("Desktop stand-in for the Android client."),
            "{h}"
        );
        assert_eq!(h, help("xx"));
    }
}
