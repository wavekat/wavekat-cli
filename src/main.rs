use anyhow::Result;
use clap::{Parser, Subcommand};

mod client;
mod commands;
mod config;
mod style;

// Short form (`wk -V`) keeps it terse; the long form (`wk --version`)
// pads in the package name and homepage so a user pasting the output
// into a bug report doesn't have to add context. clap prepends the
// bin name ("wk ") to whatever we give it, so the constants here pick
// up at the version number.
const SHORT_VERSION: &str = env!("CARGO_PKG_VERSION");
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n",
    env!("CARGO_PKG_NAME"),
    " — ",
    env!("CARGO_PKG_DESCRIPTION"),
    "\n",
    env!("CARGO_PKG_REPOSITORY"),
);

#[derive(Parser)]
#[command(
    name = "wk",
    version = SHORT_VERSION,
    long_version = LONG_VERSION,
    about = "Command-line client for the WaveKat platform",
    long_about = "Command-line client for the WaveKat platform.\n\nRun `wk login` to authenticate. \
                  Credentials are stored under your platform config dir (e.g. ~/.config/wavekat/auth.json on Linux/macOS)."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authenticate against a WaveKat platform instance
    Login(commands::login::Args),
    /// Forget stored credentials
    Logout,
    /// Show the currently signed-in user (`GET /api/me`)
    Me,
    /// Manage projects
    Projects {
        #[command(subcommand)]
        command: commands::projects::Cmd,
    },
    /// Manage annotations
    Annotations {
        #[command(subcommand)]
        command: commands::annotations::Cmd,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Login(args) => commands::login::run(args).await,
        Command::Logout => commands::logout::run().await,
        Command::Me => commands::me::run().await,
        Command::Projects { command } => commands::projects::run(command).await,
        Command::Annotations { command } => commands::annotations::run(command).await,
    }
}
