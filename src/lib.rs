mod app;
mod cli;
mod settings;

use anyhow::Result;
use clap::Parser;

/// Parses the process arguments and executes the selected command.
///
/// # Errors
///
/// Returns an error when arguments are incomplete or an external operation fails.
pub fn run() -> Result<()> {
    match cli::Cli::parse().command {
        cli::Command::Clean(options) => app::clean(&options),
        cli::Command::Refresh(options) => app::refresh(&options),
        cli::Command::Prevent => settings::prevent_attribution(),
    }
}
