use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "anticlaude",
    version,
    about = "Remove Claude/AI attribution from a GitHub repository in one command"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Option<Action>,
    /// GitHub URL or OWNER/REPO slug
    target: Option<String>,
    /// Skip confirmation
    #[arg(short = 'y', long)]
    yes: bool,
    /// Show the plan only
    #[arg(long)]
    dry_run: bool,
    /// Skip contributors graph refresh
    #[arg(long)]
    no_refresh: bool,
    /// Skip the restorable backup bundle
    #[arg(long)]
    no_backup: bool,
}

#[derive(Subcommand)]
enum Action {
    /// Disable future Claude Code attribution
    Prevent,
}

pub(crate) enum Command {
    Clean(CleanOptions),
    Prevent,
}

pub(crate) struct CleanOptions {
    pub target: String,
    pub yes: bool,
    pub dry_run: bool,
    pub refresh: bool,
    pub backup: bool,
}

impl Cli {
    pub(crate) fn into_command(self) -> Result<Command> {
        if matches!(self.command, Some(Action::Prevent)) {
            return Ok(Command::Prevent);
        }
        Ok(Command::Clean(CleanOptions {
            target: self
                .target
                .context("provide a GitHub URL or OWNER/REPO slug")?,
            yes: self.yes,
            dry_run: self.dry_run,
            refresh: !self.no_refresh,
            backup: !self.no_backup,
        }))
    }
}
