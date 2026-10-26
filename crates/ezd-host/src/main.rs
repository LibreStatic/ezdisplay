//! `ezd-host`: host side of ezdisplay.
//!
//! * `ezd-host run --source synthetic`: listen for one client at a time and stream.
//! * `ezd-host run --wifi --source synthetic`: also accept paired tablets over Wi-Fi.
//! * `ezd-host paired list|forget`: manage the tablets paired over Wi-Fi.
//! * `ezd-host diag`: print a JSON report about the machine (GPUs, encoders, adb).

mod diag;
#[cfg(target_os = "linux")]
mod firewall;
mod i18n;
mod run;
#[cfg(windows)]
mod vdd;
#[cfg(feature = "wifi")]
mod wifi;

use anyhow::Result;
use clap::{
    ArgMatches, Command as ClapCommand, CommandFactory, FromArgMatches, Parser, Subcommand,
};
use i18n_embed::fluent::FluentLanguageLoader;

// Help texts live in i18n/<lang>/ezd-host.ftl and are applied by `command`; the derive structs
// only describe the shape of the command line.
#[derive(Parser, Debug)]
#[command(name = "ezd-host", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Run(run::RunArgs),
    Diag,
    #[cfg(feature = "wifi")]
    #[command(subcommand)]
    Paired(wifi::PairedCommand),
    #[cfg(windows)]
    #[command(subcommand)]
    Vdd(vdd::VddCommand),
}

/// The command line with every help text taken from `l`.
fn command(l: &FluentLanguageLoader) -> ClapCommand {
    use i18n_embed_fl::fl;
    let cmd = Cli::command();
    let cmd = cmd
        .about(fl!(l, "about"))
        .mut_subcommand("run", |s| run::localize(s.about(fl!(l, "cmd-run")), l))
        .mut_subcommand("diag", |s| s.about(fl!(l, "cmd-diag")));
    #[cfg(feature = "wifi")]
    let cmd = cmd.mut_subcommand("paired", |s| {
        wifi::localize(s.about(fl!(l, "cmd-paired")), l)
    });
    #[cfg(windows)]
    let cmd = cmd.mut_subcommand("vdd", |s| vdd::localize(s.about(fl!(l, "cmd-vdd")), l));
    i18n::localize_version(i18n::localize_chrome(cmd, l), l)
}

fn parse(matches: &ArgMatches) -> Cli {
    Cli::from_arg_matches(matches).unwrap_or_else(|e| e.exit())
}

fn main() -> Result<()> {
    i18n::init();
    #[cfg(windows)]
    ezd_host_windows::isolate_std_handles();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let matches = command(&i18n::LANGUAGE_LOADER).get_matches();
    match parse(&matches).command {
        Command::Run(args) => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            rt.block_on(run::run(args))
        }
        Command::Diag => {
            println!("{}", serde_json::to_string_pretty(&diag::report())?);
            Ok(())
        }
        #[cfg(feature = "wifi")]
        Command::Paired(cmd) => wifi::run(cmd),
        #[cfg(windows)]
        Command::Vdd(cmd) => vdd::run(cmd),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn help(lang: i18n_embed::unic_langid::LanguageIdentifier, sub: &[&str]) -> String {
        let l = i18n::loader_for(&[lang]);
        let mut args = vec!["ezd-host"];
        args.extend(sub);
        args.push("--help");
        let err = command(&l).try_get_matches_from(args).unwrap_err();
        err.render().to_string()
    }

    #[test]
    fn spanish_help_is_localized() {
        let top = help("es".parse().unwrap(), &[]);
        assert!(top.contains("Host de ezdisplay"), "{top}");
        assert!(top.contains("Uso:"), "{top}");
        assert!(top.contains("Comandos:"), "{top}");
        assert!(top.contains("Opciones:"), "{top}");
        assert!(
            !top.contains("Usage:") && !top.contains("Print help"),
            "{top}"
        );
        let run = help("es".parse().unwrap(), &["run"]);
        assert!(run.contains("Espera a un cliente"), "{run}");
        assert!(run.contains("Fuente de video:"), "{run}");
    }

    #[test]
    fn english_is_the_fallback() {
        let top = help("en".parse().unwrap(), &[]);
        assert!(top.contains("Usage:") && top.contains("Commands:"), "{top}");
        let unknown = help("xx".parse().unwrap(), &[]);
        assert_eq!(top, unknown);
    }
}
