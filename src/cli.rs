use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "anticlaude",
    version,
    about = "Safely remove Claude attribution from Git history"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Inspect or remove Claude attribution from repository history
    Clean(CleanOptions),
    /// Experimentally refresh GitHub's cached Contributors graph
    Refresh(RefreshOptions),
    /// Disable future Claude Code attribution locally
    Prevent,
}

#[derive(Args)]
pub(crate) struct CleanOptions {
    /// GitHub URL or OWNER/REPO slug
    pub target: String,
    /// Skip the destructive-operation confirmation
    #[arg(short = 'y', long)]
    pub yes: bool,
    /// Show affected commits and refs without changing anything
    #[arg(long)]
    pub dry_run: bool,
    /// Skip the restorable backup bundle
    #[arg(long)]
    pub no_backup: bool,
}

#[derive(Args)]
pub(crate) struct RefreshOptions {
    /// GitHub URL or OWNER/REPO slug
    pub target: String,
    /// Confirm the experimental branch rename and empty commit
    #[arg(short = 'y', long)]
    pub yes: bool,
}
