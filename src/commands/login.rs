// `wk login` — loopback OAuth flow.
//
// The actual handshake (loopback listener, CSRF state, callback parsing,
// browser-friendly response page) lives in `wavekat-platform-client`.
// This file is the CLI shell around it: argument parsing, deciding
// whether to open the browser, persisting the resulting token, and
// printing a confirmation that includes the user's role.
//
// `--no-browser` falls back to printing the URL for the user to open
// manually — useful on a remote host where no browser is available.
// `--token` skips the dance entirely (e.g. for CI), accepting a
// pre-minted `wk_…` token.

use anyhow::{bail, Context, Result};
use clap::Args as ClapArgs;
use wavekat_platform_client::{loopback_handshake, HandshakeOptions};

use crate::client::Client;
use crate::config;
use crate::style;

pub const DEFAULT_BASE_URL: &str = "https://platform.wavekat.com";

#[derive(ClapArgs)]
pub struct Args {
    /// Base URL of the WaveKat platform (e.g. https://platform.wavekat.com).
    /// If omitted, the previously stored value is reused, then the public
    /// platform URL.
    #[arg(long, env = "WK_BASE_URL")]
    base_url: Option<String>,

    /// Skip opening the browser; print the URL instead. Useful on a remote
    /// host. The CLI still listens on a loopback port — open the URL on
    /// any browser that can reach this machine on that port (typically via
    /// SSH port-forward).
    #[arg(long)]
    no_browser: bool,

    /// Pre-minted `wk_…` bearer token. Skips the browser handshake
    /// entirely and just verifies + saves the token. Intended for CI.
    /// Read from `WK_TOKEN` if set.
    #[arg(long, env = "WK_TOKEN")]
    token: Option<String>,
}

pub async fn run(args: Args) -> Result<()> {
    let existing = config::load().ok();

    let base_url = args
        .base_url
        .or_else(|| existing.as_ref().map(|c| c.base_url.clone()))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
        .trim_end_matches('/')
        .to_string();

    let token = match args.token {
        Some(t) => t.trim().to_string(),
        None => browser_handshake(&base_url, args.no_browser).await?,
    };
    if token.is_empty() {
        bail!("got an empty token from the platform");
    }

    // Merge into the existing config so we preserve `install_id` and
    // any telemetry preference across re-logins.
    let mut cfg = config::load_or_default();
    cfg.base_url = base_url;
    cfg.token = Some(token);
    cfg.session_cookie = None;

    // Verify against /api/me before persisting — keeps a typo or a
    // half-broken handshake from poisoning the saved config.
    let client = Client::new(&cfg)?;
    let me: serde_json::Value = client
        .get_json("/api/me")
        .await
        .context("verifying token against /api/me")?;
    let login = me.get("login").and_then(|v| v.as_str()).unwrap_or("?");
    let role = me.get("role").and_then(|v| v.as_str()).unwrap_or("?");

    config::save(&cfg)?;
    let path = config::auth_path()?;
    println!(
        "{} Signed in as {} ({} {}).",
        style::green("✓"),
        style::bold(login),
        style::dim("role:"),
        style::role(role),
    );
    println!(
        "{} {}",
        style::dim("Credentials saved to"),
        style::dim(&path.display().to_string()),
    );
    Ok(())
}

async fn browser_handshake(base_url: &str, no_browser: bool) -> Result<String> {
    // `client = "wavekat-cli"` is the consent-screen title; `source`
    // defaults to the machine's hostname (rendered as "from <host>").
    // Timeout matches the CLI's previous 5-minute deadline — generous
    // enough that re-running `wk login` is the right answer if a user
    // gets distracted.
    let options = HandshakeOptions {
        client: Some("wavekat-cli".to_string()),
        ..HandshakeOptions::default()
    };
    let pending =
        loopback_handshake(base_url, options).context("starting loopback OAuth handshake")?;

    if no_browser {
        println!(
            "Open this URL in any browser to finish signing in:\n  {}\n",
            pending.url(),
        );
    } else {
        println!("Opening {base_url} in your browser to sign in…");
        if let Err(e) = webbrowser::open(pending.url()) {
            eprintln!("(couldn't open the browser automatically: {e})");
            println!("Open this URL manually:\n  {}\n", pending.url());
        }
    }
    println!("Waiting for the browser to redirect back (Ctrl-C to cancel)…");

    let outcome = pending
        .wait()
        .await
        .context("waiting for the browser callback")?;
    Ok(outcome.token.as_str().to_string())
}
