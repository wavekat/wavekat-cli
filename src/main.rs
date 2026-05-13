use anyhow::Result;
use clap::{Parser, Subcommand};

mod audio;
mod client;
mod commands;
mod config;
mod progress;
mod style;
mod telemetry;

// Both `-V` and `--version` print the same string — that matches what
// `rustc -V` / `cargo -V` actually do, despite clap's default of
// splitting them. clap prepends the bin name ("wk ") for us.
const VERSION: &str = concat!(
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
    version = VERSION,
    about = "Command-line client for the WaveKat platform",
    long_about = "Command-line client for the WaveKat platform.\n\n\
                  Run `wk login` to authenticate. Credentials are stored under your platform \
                  config dir (e.g. ~/.config/wavekat/auth.json on Linux/macOS).\n\n\
                  Run `wk update` to upgrade in place, or `wk agents` for the AI-agent \
                  integration guide (also at https://github.com/wavekat/wavekat-cli/blob/main/AGENTS.md)."
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
    /// Manage dataset exports
    Exports {
        #[command(subcommand)]
        command: commands::exports::Cmd,
    },
    /// Manage trained models (push, list, download)
    Models {
        #[command(subcommand)]
        command: commands::models::Cmd,
    },
    /// Manage project files (list, reserve / unreserve test set)
    Files {
        #[command(subcommand)]
        command: commands::files::Cmd,
    },
    /// Read or change persisted CLI preferences (telemetry, …)
    Config {
        #[command(subcommand)]
        command: commands::config::Cmd,
    },
    /// Print the local CLI version and probe the platform's `/api/health`
    Version(commands::version::Args),
    /// Replace this binary with the latest release (or `--check` to peek)
    Update(commands::update::Args),
    /// Print the bundled AGENTS.md guide for AI agents using `wk`
    Agents,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Telemetry must be initialized before anything fallible so a
    // panic during parsing still gets captured. The guard's `Drop`
    // flushes pending events on exit (with a short timeout).
    let _telemetry = telemetry::init();

    let cli = Cli::parse();
    let command_name = command_name(&cli.command);

    // First-run notice goes to stderr after parsing succeeded —
    // before any command output — so it can't garble JSON streams.
    telemetry::maybe_print_first_run_notice();

    let started = std::time::Instant::now();
    let result = dispatch(cli.command).await;
    if let Err(err) = &result {
        telemetry::report_error(command_name, started.elapsed(), err);
    }
    result
}

async fn dispatch(cmd: Command) -> Result<()> {
    match cmd {
        Command::Login(args) => commands::login::run(args).await,
        Command::Logout => commands::logout::run().await,
        Command::Me => commands::me::run().await,
        Command::Projects { command } => commands::projects::run(command).await,
        Command::Annotations { command } => commands::annotations::run(command).await,
        Command::Exports { command } => commands::exports::run(command).await,
        Command::Models { command } => commands::models::run(command).await,
        Command::Files { command } => commands::files::run(command).await,
        Command::Config { command } => commands::config::run(command).await,
        Command::Version(args) => commands::version::run(args).await,
        Command::Update(args) => commands::update::run(args).await,
        Command::Agents => commands::agents::run().await,
    }
}

/// Static string for the `cli.command` tag — keeps the cardinality
/// fixed (no argv, no flag values) so Sentry can group cleanly.
fn command_name(cmd: &Command) -> &'static str {
    match cmd {
        Command::Login(_) => "login",
        Command::Logout => "logout",
        Command::Me => "me",
        Command::Projects { .. } => "projects",
        Command::Annotations { .. } => "annotations",
        Command::Exports { .. } => "exports",
        Command::Models { .. } => "models",
        Command::Files { .. } => "files",
        Command::Config { .. } => "config",
        Command::Version(_) => "version",
        Command::Update(_) => "update",
        Command::Agents => "agents",
    }
}
