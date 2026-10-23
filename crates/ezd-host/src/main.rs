//! `ezd-host`: host side of ezdisplay.
//!
//! * `ezd-host run --source synthetic`: listen for one client at a time and stream.
//! * `ezd-host diag`: print a JSON report about the machine (GPUs, encoders, adb).

mod diag;
mod platform;
mod run;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "ezd-host", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Listen for a client and stream video to it.
    Run(run::RunArgs),
    /// Print a JSON diagnostics report (failures are reported as data).
    Diag,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().command {
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
    }
}
