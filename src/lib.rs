mod app;
mod cli;

use anyhow::Result;
use clap::Parser;

pub fn run() -> Result<()> {
    match cli::Cli::parse().into_command()? {
        cli::Command::Clean(options) => app::execute(options),
        cli::Command::Prevent => app::prevent(),
    }
}
