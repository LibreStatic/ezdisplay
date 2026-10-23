//! `ezd-host vdd`: manage the Virtual Display Driver used by the Windows backend.

use anyhow::{Result, anyhow};
use clap::Subcommand;
use ezd_host_windows::vdd_xml::Mode;

#[derive(Subcommand, Debug)]
pub enum VddCommand {
    /// Print the state of the VDD installation as JSON (changes nothing).
    Status,
    /// One-time setup, run as administrator: lets normal users edit the VDD settings,
    /// optionally assigns the GPU and pre-adds modes, then reloads the driver.
    Setup {
        /// GPU the virtual display should render on (adapter name, as in `ezd-host diag`).
        #[arg(long)]
        gpu: Option<String>,
        /// Mode to offer, `WIDTHxHEIGHT@HZ`; repeatable.
        #[arg(long = "mode", value_name = "WxH@Hz")]
        modes: Vec<String>,
    },
    /// Undo the permission change made by `setup`.
    Revert,
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
