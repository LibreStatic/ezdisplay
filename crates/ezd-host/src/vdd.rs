//! `ezd-host vdd`: manage the Virtual Display Driver used by the Windows backend.

use anyhow::{Result, anyhow};
use clap::{Command, Subcommand};
use ezd_host_windows::vdd_install::{InstallError, InstallOutcome, UninstallOutcome};
use ezd_host_windows::vdd_xml::Mode;
use i18n_embed::fluent::FluentLanguageLoader;
use std::path::PathBuf;

use crate::i18n::fl;

/// Exit code of `install`/`uninstall` when the operation worked but Windows needs a reboot
/// (ERROR_SUCCESS_REBOOT_REQUIRED); the installer reads it.
const EXIT_REBOOT_REQUIRED: i32 = 3010;

// Help texts live in i18n/<lang>/ezd-host.ftl (`vdd-*` ids); see `localize`.
#[derive(Subcommand, Debug)]
pub enum VddCommand {
    Status,
    Setup {
        #[arg(long)]
        gpu: Option<String>,
        #[arg(long = "mode", value_name = "WxH@Hz")]
        modes: Vec<String>,
    },
    Revert,
    /// Installs the bundled driver (administrator).
    Install {
        #[arg(long, value_name = "DIR")]
        driver_dir: PathBuf,
    },
    /// Removes the driver installed by `install` (administrator).
    Uninstall {
        #[arg(long, value_name = "DIR")]
        driver_dir: Option<PathBuf>,
    },
}

/// Applies the localized help of the `vdd` subcommands and their options.
pub fn localize(cmd: Command, l: &FluentLanguageLoader) -> Command {
    use i18n_embed_fl::fl;
    cmd.mut_subcommand("status", |s| s.about(fl!(l, "vdd-status")))
        .mut_subcommand("setup", |s| {
            s.about(fl!(l, "vdd-setup"))
                .mut_arg("gpu", |a| a.help(fl!(l, "vdd-setup-gpu")))
                .mut_arg("modes", |a| a.help(fl!(l, "vdd-setup-mode")))
        })
        .mut_subcommand("revert", |s| s.about(fl!(l, "vdd-revert")))
        .mut_subcommand("install", |s| {
            s.about(fl!(l, "vdd-install"))
                .mut_arg("driver_dir", |a| a.help(fl!(l, "vdd-install-driver-dir")))
        })
        .mut_subcommand("uninstall", |s| {
            s.about(fl!(l, "vdd-uninstall"))
                .mut_arg("driver_dir", |a| a.help(fl!(l, "vdd-uninstall-driver-dir")))
        })
}

pub fn run(cmd: VddCommand) -> Result<()> {
    match cmd {
        VddCommand::Status => {
            println!(
                "{}",
                serde_json::to_string_pretty(&ezd_host_windows::vdd_status())?
            );
        }
        VddCommand::Setup { gpu, modes } => {
            let modes = modes
                .iter()
                .map(|m| Mode::parse(m).map_err(|e| anyhow!("{e}")))
                .collect::<Result<Vec<_>>>()?;
            print_lines(ezd_host_windows::vdd_setup(gpu.as_deref(), &modes))?;
        }
        VddCommand::Revert => print_lines(ezd_host_windows::vdd_revert())?,
        VddCommand::Install { driver_dir } => {
            let r = ezd_host_windows::vdd_install(&driver_dir);
            finish(r.map(|o| match o {
                InstallOutcome::Installed {
                    reboot_required: false,
                } => (fl!("vdd-installed"), false),
                InstallOutcome::Installed {
                    reboot_required: true,
                } => (fl!("vdd-installed-reboot"), true),
                InstallOutcome::PreexistingLeftAlone => (fl!("vdd-preexisting"), false),
            }));
        }
        VddCommand::Uninstall { driver_dir } => {
            let r = ezd_host_windows::vdd_uninstall(driver_dir.as_deref());
            finish(r.map(|o| match o {
                UninstallOutcome::Removed {
                    reboot_required: false,
                } => (fl!("vdd-removed"), false),
                UninstallOutcome::Removed {
                    reboot_required: true,
                } => (fl!("vdd-removed-reboot"), true),
                UninstallOutcome::NotOurs => (fl!("vdd-not-ours"), false),
            }));
        }
    }
    Ok(())
}

fn print_lines(r: Result<Vec<String>, String>) -> Result<()> {
    for l in r.map_err(|e| anyhow!(e))? {
        println!("{l}");
    }
    Ok(())
}

/// Prints the outcome and exits with the code the installer expects: 0 ok, 3010 reboot
/// required, 1 error (message on stderr).
fn finish(r: Result<(String, bool), InstallError>) -> ! {
    match r {
        Ok((msg, reboot)) => {
            println!("{msg}");
            std::process::exit(if reboot { EXIT_REBOOT_REQUIRED } else { 0 });
        }
        Err(e) => {
            let msg = match e {
                InstallError::NotElevated => fl!("vdd-err-not-elevated"),
                InstallError::Failed(detail) => fl!("vdd-err-failed", detail = detail),
            };
            eprintln!("{msg}");
            std::process::exit(1);
        }
    }
}
