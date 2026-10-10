use anyhow::Result;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

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

// clap has no first-class way to group subcommands under headings in
// the top-level help, so we render the command list ourselves via
// `help_template` and replace clap's default `{subcommands}` block.
// Keep this in sync with the `Command` enum below.
const HELP_HEAD: &str = "\
{about-with-newline}
{usage-heading} {usage}

Account:
  login        Authenticate against a WaveKat platform instance
  logout       Forget stored credentials
  me           Show the currently signed-in user (`GET /api/me`)

Resources:
  projects     Manage projects
  annotations  Manage annotations
  exports      Manage dataset exports
  models       Manage trained models (push, list, download)
  files        Manage project files (list, reserve / unreserve test set)

";

// Only shown to accounts whose cached role is `root` — see `show_admin`.
const HELP_ADMIN: &str = "\
Admin & raw API:
  admin        Root-only platform analytics and fleet tags (users, installs, usage, …) as JSON
";

const HELP_RAW_API: &str = "\
Raw API:
";

const HELP_TAIL: &str =
    "  api          GET any platform endpoint and print its JSON (`wk api /api/me`)

CLI:
  config       Read or change persisted CLI preferences (telemetry, …)
  version      Print the local CLI version and probe the platform's `/api/health`
  update       Replace this binary with the latest release (or `--check` to peek)
  agents       Print the bundled AGENTS.md guide for AI agents using `wk`
  help         Print this message or the help of the given subcommand(s)

Options:
{options}{after-help}";

fn help_template(show_admin: bool) -> String {
    let section = if show_admin { HELP_ADMIN } else { HELP_RAW_API };
    format!("{HELP_HEAD}{section}{HELP_TAIL}")
}

/// Whether `wk admin` is listed in help. Only root accounts can use it,
/// so everyone else shouldn't see it. This reads the role cached by
/// `wk login` / `wk me` — no network call — and is cosmetic only: the
/// command stays callable and the platform still enforces the role.
fn show_admin(cfg: &config::AuthConfig) -> bool {
    cfg.role.as_deref() == Some("root")
}

/// The clap command with help shaped for the signed-in account.
fn cli_command(show_admin: bool) -> clap::Command {
    Cli::command()
        .help_template(help_template(show_admin))
        .mut_subcommand("admin", |c| c.hide(!show_admin))
}

#[derive(Parser)]
#[command(
    name = "wk",
    version = VERSION,
    about = "Command-line client for the WaveKat platform",
    long_about = "Command-line client for the WaveKat platform.\n\n\
                  Run `wk login` to authenticate. Credentials are stored under your platform \
                  config dir (e.g. ~/.config/wavekat/auth.json on Linux/macOS).\n\n\
                  Run `wk update` to upgrade in place, or `wk agents` for the AI-agent \
                  integration guide (also at https://github.com/wavekat/wavekat-cli/blob/main/AGENTS.md).",
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
    Me(commands::me::Args),
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
    /// Root-only platform analytics and fleet tags (users, installs, usage, …) as JSON
    Admin {
        #[command(subcommand)]
        command: commands::admin::Cmd,
    },
    /// GET any platform endpoint and print its JSON (`wk api /api/me`)
    Api(commands::api::Args),
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

    let show_admin = show_admin(&config::load_or_default());
    let cli =
        Cli::from_arg_matches(&cli_command(show_admin).get_matches()).unwrap_or_else(|e| e.exit());
    let command_name = command_name(&cli.command);

    // First-run notice goes to stderr after parsing succeeded —
    // before any command output — so it can't garble JSON streams.
    telemetry::maybe_print_first_run_notice();

    let started = std::time::Instant::now();
    let result = dispatch(cli.command).await;
    if let Err(err) = &result {
        if reports_errors(command_name) {
            telemetry::report_error(command_name, started.elapsed(), err);
        }
    }
    result.map_err(explain_network_error)
}

/// Put a plain-language headline on errors where the request never
/// reached the server (DNS failure, refused connection, timeout). The
/// raw reqwest/hyper chain (`client error (Connect)` → `dns error` →
/// `failed to lookup address information: Try again`) reads like a
/// `wk` bug when it's really the network; the original chain stays
/// underneath as "Caused by" for debugging.
fn explain_network_error(err: anyhow::Error) -> anyhow::Error {
    let Some(req_err) = err
        .chain()
        .find_map(|e| e.downcast_ref::<reqwest::Error>())
        .filter(|e| e.is_connect() || e.is_timeout())
    else {
        return err;
    };
    let host = req_err
        .url()
        .and_then(|u| u.host_str())
        .unwrap_or("the server")
        .to_string();
    let dns = err.chain().any(|e| e.to_string().contains("dns error"));
    let what = if dns {
        format!("couldn't look up {host} (DNS failed)")
    } else if req_err.is_timeout() {
        format!("timed out reaching {host}")
    } else {
        format!("couldn't connect to {host}")
    };
    err.context(format!(
        "{what} — check your network connection (Wi-Fi, VPN, proxy) and try again"
    ))
}

async fn dispatch(cmd: Command) -> Result<()> {
    match cmd {
        Command::Login(args) => commands::login::run(args).await,
        Command::Logout => commands::logout::run().await,
        Command::Me(args) => commands::me::run(args).await,
        Command::Projects { command } => commands::projects::run(command).await,
        Command::Annotations { command } => commands::annotations::run(command).await,
        Command::Exports { command } => commands::exports::run(command).await,
        Command::Models { command } => commands::models::run(command).await,
        Command::Files { command } => commands::files::run(command).await,
        Command::Admin { command } => commands::admin::run(command).await,
        Command::Api(args) => commands::api::run(args).await,
        Command::Config { command } => commands::config::run(command).await,
        Command::Version(args) => commands::version::run(args).await,
        Command::Update(args) => commands::update::run(args).await,
        Command::Agents => commands::agents::run().await,
    }
}

/// `admin` and `api` errors carry caller-chosen URLs and server bodies
/// about customers (user ids, `-q q=<email>` searches) that the
/// telemetry scrubber can't recognise as identifiers, so they are kept
/// out of error reporting entirely. Panics are still reported.
fn reports_errors(command_name: &str) -> bool {
    !matches!(command_name, "admin" | "api")
}

/// Static string for the `cli.command` tag — keeps the cardinality
/// fixed (no argv, no flag values) so Sentry can group cleanly.
fn command_name(cmd: &Command) -> &'static str {
    match cmd {
        Command::Login(_) => "login",
        Command::Logout => "logout",
        Command::Me(_) => "me",
        Command::Projects { .. } => "projects",
        Command::Annotations { .. } => "annotations",
        Command::Exports { .. } => "exports",
        Command::Models { .. } => "models",
        Command::Files { .. } => "files",
        Command::Admin { .. } => "admin",
        Command::Api(_) => "api",
        Command::Config { .. } => "config",
        Command::Version(_) => "version",
        Command::Update(_) => "update",
        Command::Agents => "agents",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_help(show_admin: bool) -> String {
        cli_command(show_admin).render_help().to_string()
    }

    #[test]
    fn admin_listed_only_for_root() {
        let root = render_help(true);
        assert!(root.contains("Admin & raw API:"), "{root}");
        assert!(root.contains("  admin "), "{root}");

        let user = render_help(false);
        assert!(!user.contains("admin"), "{user}");
        assert!(user.contains("Raw API:"), "{user}");
        assert!(user.contains("  api "), "{user}");
    }

    #[test]
    fn show_admin_needs_cached_root_role() {
        let with_role = |role: Option<&str>| config::AuthConfig {
            role: role.map(str::to_string),
            ..Default::default()
        };
        assert!(show_admin(&with_role(Some("root"))));
        assert!(!show_admin(&with_role(Some("user"))));
        assert!(!show_admin(&with_role(None)));
    }

    #[test]
    fn hidden_admin_still_parses() {
        let m = cli_command(false)
            .try_get_matches_from(["wk", "admin", "geo"])
            .unwrap();
        let cli = Cli::from_arg_matches(&m).unwrap();
        assert_eq!(command_name(&cli.command), "admin");
    }

    #[test]
    fn customer_data_commands_skip_error_reporting() {
        assert!(!reports_errors("admin"));
        assert!(!reports_errors("api"));
        assert!(reports_errors("exports"));
    }

    #[tokio::test]
    async fn connect_errors_get_network_headline() {
        // Nothing listens on port 1 locally, so this fails at connect.
        let raw = reqwest::get("http://127.0.0.1:1/").await.unwrap_err();
        let err = explain_network_error(anyhow::Error::new(raw).context("GET http://127.0.0.1:1/"));
        assert_eq!(
            err.to_string(),
            "couldn't connect to 127.0.0.1 — check your network connection (Wi-Fi, VPN, proxy) and try again"
        );
    }

    #[tokio::test]
    async fn dns_errors_say_dns() {
        // `.invalid` is reserved (RFC 6761) and never resolves.
        let raw = reqwest::get("http://wk-test.invalid/").await.unwrap_err();
        let err = explain_network_error(anyhow::Error::new(raw));
        assert_eq!(
            err.to_string(),
            "couldn't look up wk-test.invalid (DNS failed) — check your network connection (Wi-Fi, VPN, proxy) and try again"
        );
    }

    #[test]
    fn other_errors_pass_through() {
        let err = explain_network_error(anyhow::anyhow!("boom"));
        assert_eq!(err.to_string(), "boom");
    }
}
