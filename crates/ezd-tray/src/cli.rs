//! Command line.

use crate::i18n;
use crate::ipc;
use clap::{Arg, ArgAction, Command, ValueEnum};
use i18n_embed::fluent::FluentLanguageLoader;
use i18n_embed_fl::fl;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OnOff {
    On,
    Off,
}

#[derive(Debug, Clone)]
pub struct Args {
    /// Open the settings window.
    pub settings: bool,
    /// Ask the running instance to quit and wait for it.
    pub quit: bool,
    /// Turn the autostart entry on or off and exit.
    pub autostart: Option<OnOff>,
    /// Register the launcher task to run elevated (or not) and exit.
    pub elevation: Option<OnOff>,
    /// Another config file (tests, manual runs).
    pub config: Option<PathBuf>,
}

/// How long `--quit` waits for the running instance to stop its service and exit.
const QUIT_WAIT: Duration = Duration::from_secs(15);

pub fn command(l: &FluentLanguageLoader) -> Command {
    let cmd = Command::new("ezdisplay")
        .version(env!("CARGO_PKG_VERSION"))
        .about(fl!(l, "about"))
        .arg(
            Arg::new("settings")
                .long("settings")
                .action(ArgAction::SetTrue)
                .help(fl!(l, "arg-settings")),
        )
        .arg(
            Arg::new("quit")
                .long("quit")
                .action(ArgAction::SetTrue)
                .help(fl!(l, "arg-quit")),
        )
        .arg(
            Arg::new("autostart")
                .long("autostart")
                .value_name("on|off")
                .value_parser(clap::value_parser!(OnOff))
                .help(fl!(l, "arg-autostart")),
        )
        .arg(
            Arg::new("elevation")
                .long("elevation")
                .value_name("on|off")
                .value_parser(clap::value_parser!(OnOff))
                .hide(!cfg!(windows))
                .help(fl!(l, "arg-elevation")),
        )
        .arg(
            Arg::new("config")
                .long("config")
                .value_name("PATH")
                .value_parser(clap::value_parser!(PathBuf))
                .hide(true),
        );
    i18n::localize_chrome(cmd, l)
}

pub fn parse(l: &FluentLanguageLoader) -> Args {
    from_matches(&command(l).get_matches())
}

fn from_matches(m: &clap::ArgMatches) -> Args {
    Args {
        settings: m.get_flag("settings"),
        quit: m.get_flag("quit"),
        autostart: m.get_one::<OnOff>("autostart").copied(),
        elevation: m.get_one::<OnOff>("elevation").copied(),
        config: m.get_one::<PathBuf>("config").cloned(),
    }
}

/// A short stable tag for a path (FNV-1a), used to give runs with another config their own
/// instance name.
pub fn path_tag(p: &std::path::Path) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in p.to_string_lossy().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

/// `--quit`: asks the running instance to stop and waits until it has. Succeeds when none runs.
pub fn quit_running(socket: &str) -> ExitCode {
    match ipc::send(socket, ipc::Command::Quit, QUIT_WAIT) {
        Ok(()) => {
            // The instance replies just before its process ends; give it a moment to let go of
            // its files (the installer replaces them next).
            std::thread::sleep(Duration::from_millis(500));
            ExitCode::SUCCESS
        }
        Err(ipc::SendError::NoInstance) => ExitCode::SUCCESS,
        Err(ipc::SendError::Timeout) => {
            eprintln!("timed out waiting for ezdisplay to quit");
            ExitCode::FAILURE
        }
        Err(ipc::SendError::Io(e)) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::loader_for;

    fn help(lang: &str) -> String {
        let l = loader_for(&[lang.parse().unwrap()]);
        command(&l)
            .try_get_matches_from(["ezdisplay", "--help"])
            .unwrap_err()
            .render()
            .to_string()
    }

    #[test]
    fn flags_parse() {
        let l = loader_for(&[]);
        let m = command(&l)
            .try_get_matches_from([
                "ezdisplay",
                "--settings",
                "--autostart",
                "off",
                "--elevation",
                "on",
                "--config",
                "/x",
            ])
            .unwrap();
        let a = from_matches(&m);
        assert!(a.settings && !a.quit);
        assert_eq!(a.autostart, Some(OnOff::Off));
        assert_eq!(a.elevation, Some(OnOff::On));
        assert_eq!(a.config, Some(PathBuf::from("/x")));
    }

    #[test]
    fn help_is_localized_and_hides_config() {
        let en = help("en");
        assert!(en.contains("Usage:") && en.contains("--quit"), "{en}");
        assert!(!en.contains("--config"), "{en}");
        let es = help("es");
        assert!(es.contains("Uso:") && !es.contains("Usage:"), "{es}");
    }

    #[test]
    fn path_tags_differ() {
        assert_ne!(
            path_tag(std::path::Path::new("/a")),
            path_tag(std::path::Path::new("/b"))
        );
    }
}
