//! `ezd-host vdd`: manage the Virtual Display Driver used by the Windows backend.

use anyhow::{Result, anyhow};
use clap::{Command, Subcommand};
use ezd_host_windows::vdd_xml::Mode;
use i18n_embed::fluent::FluentLanguageLoader;

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
    }
    Ok(())
}

fn print_lines(r: Result<Vec<String>, String>) -> Result<()> {
    for l in r.map_err(|e| anyhow!(e))? {
        println!("{l}");
    }
    Ok(())
}
