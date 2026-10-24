//! `ezd-fake-client`: connect to an `ezd-host`, decode the stream, print latency statistics.

use anyhow::{Context, Result};
use clap::{ArgMatches, Command, CommandFactory, FromArgMatches, Parser, ValueEnum};
use ezd_fake_client::fl;
use ezd_fake_client::i18n;
use ezd_fake_client::{FakeClientConfig, InputScript, run_fake_client};
use ezd_link::Link;
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
    #[arg(long, hide = true)]
    proto_minor: Option<u16>,
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
        });
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
    let dump_frame = match a.dump_frame {
        Some(v) => {
            let n: u64 = v[0].parse().with_context(|| fl!("err-dump-frame"))?;
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
