use anyhow::{anyhow, Context, Result};
use clap::Args as ClapArgs;
use std::io::{self, Write};

use crate::client::Client;
use crate::config::{self, AuthConfig};

/// `wk login` arguments.
///
/// In v1 we don't have a CLI-friendly auth endpoint on the platform yet, so
/// the simplest path is "paste your session cookie". A proper device-code
/// flow lands together with the export feature on the platform side.
#[derive(ClapArgs)]
pub struct Args {
    /// Base URL of the WaveKat platform (e.g. https://platform.wavekat.com).
    /// If omitted, the previously stored value is reused, or you'll be prompted.
    #[arg(long, env = "WK_BASE_URL")]
    base_url: Option<String>,

    /// Value of the `wk_session` cookie. Read from `WK_SESSION` if set,
    /// otherwise prompted for interactively (input is hidden).
    #[arg(long, env = "WK_SESSION")]
    session: Option<String>,
}

pub async fn run(args: Args) -> Result<()> {
    let existing = config::load().ok();

    let base_url = args
        .base_url
        .or_else(|| existing.as_ref().map(|c| c.base_url.clone()))
        .map(|s| s.trim_end_matches('/').to_string())
        .map(Ok)
        .unwrap_or_else(|| prompt_line("Base URL"))?;

    println!(
        "\nTo sign in:\n  1. Open {base_url} in your browser and sign in via GitHub.\n  2. Open dev tools → Application → Cookies → {base_url}.\n  3. Copy the value of the `wk_session` cookie.\n"
    );

    let session = match args.session {
        Some(s) => s,
        None => rpassword::prompt_password("wk_session cookie value: ")
            .context("reading session cookie")?,
    };
    let session = session.trim().to_string();
    if session.is_empty() {
        return Err(anyhow!("session cookie cannot be empty"));
    }

    let cfg = AuthConfig {
        base_url,
        session_cookie: session,
    };

    // Verify the cookie before persisting it. /api/me is the cheapest
    // identity check the platform exposes and matches what the web app uses.
    let client = Client::new(&cfg)?;
    let me: serde_json::Value = client
        .get_json("/api/me")
        .await
        .context("verifying credentials against /api/me")?;
    let login = me.get("login").and_then(|v| v.as_str()).unwrap_or("?");
    let role = me.get("role").and_then(|v| v.as_str()).unwrap_or("?");

    config::save(&cfg)?;
    let path = config::auth_path()?;
    println!("Signed in as {login} (role: {role}).");
    println!("Credentials saved to {}", path.display());
    Ok(())
}

fn prompt_line(label: &str) -> Result<String> {
    print!("{label}: ");
    io::stdout().flush()?;
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    let buf = buf.trim().to_string();
    if buf.is_empty() {
        return Err(anyhow!("{label} cannot be empty"));
    }
    Ok(buf)
}
