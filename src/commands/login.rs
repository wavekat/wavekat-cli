// `wk login` — loopback OAuth flow.
//
// The actual handshake (loopback listener, CSRF state, callback parsing,
// browser-friendly response page) lives in `wavekat-platform-client`.
// This file is the CLI shell around it: argument parsing, deciding
// whether to open the browser, persisting the resulting token, and
// printing a confirmation that includes the user's role.
//
// On a remote host (SSH session, or Linux with no display) — or with
// `--no-browser` — a browser's redirect to `127.0.0.1` can't reach this
// machine, so we skip the loopback listener entirely. We print a
// `/cli-login?mode=code` URL; after the user authorizes, the page shows
// a one-time code, which they paste here. We trade code + our `state`
// for a token at `/api/auth/cli/codes/exchange`. `--token` skips the
// dance entirely (e.g. for CI), accepting a pre-minted `wk_…` token.

use std::io::Write;

use anyhow::{bail, Context, Result};
use clap::Args as ClapArgs;
use rand::distributions::{Alphanumeric, DistString};
use reqwest::Url;
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

    /// Skip opening the browser; print a URL instead. Implied when
    /// running over SSH or on Linux without a display. Open the URL in any
    /// browser, authorize, then paste the one-time code the page shows.
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
        // Telemetry can write the config file before the first login,
        // leaving `base_url` empty — treat that as unset.
        .or_else(|| existing.as_ref().map(|c| c.base_url.clone()))
        .filter(|u| !u.trim().is_empty())
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
    if no_browser || is_remote_session() {
        return code_handshake(base_url).await;
    }

    // `client = "wavekat-cli"` is the consent-screen title; `source`
    // defaults to the machine's hostname (rendered as "from <host>").
    // Timeout matches the CLI's previous 5-minute deadline — generous
    // enough that re-running `wk login` is the right answer if a user
    // gets distracted.
    let options = HandshakeOptions {
        client: Some(CLIENT.to_string()),
        ..HandshakeOptions::default()
    };
    let pending =
        loopback_handshake(base_url, options).context("starting loopback OAuth handshake")?;

    println!("Opening {base_url} in your browser to sign in…");
    if let Err(e) = webbrowser::open(pending.url()) {
        eprintln!("(couldn't open the browser automatically: {e})");
        return code_handshake(base_url).await;
    }

    println!("Waiting for the browser to redirect back (Ctrl-C to cancel)…");
    let outcome = pending
        .wait()
        .await
        .context("waiting for the browser callback")?;
    Ok(outcome.token.as_str().to_string())
}

const CLIENT: &str = "wavekat-cli";

/// Copy-and-paste sign-in: the page shows a one-time code instead of
/// redirecting to a loopback address this machine can't serve.
async fn code_handshake(base_url: &str) -> Result<String> {
    let state = Alphanumeric.sample_string(&mut rand::thread_rng(), 32);
    let url = code_login_url(base_url, &state, hostname().as_deref())?;
    println!("Open this URL in a browser on any machine to sign in:\n\n  {url}\n");
    println!("After you authorize, the page shows a one-time code. Paste it here.\n");

    let http = reqwest::Client::builder()
        .user_agent(concat!("wavekat-cli/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let exchange = format!("{base_url}/api/auth/cli/codes/exchange");
    loop {
        print!("Code: ");
        let _ = std::io::stdout().flush();
        let line = tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).map(|n| (n, line))
        })
        .await??;
        let code = match line {
            (0, _) => bail!("sign-in cancelled (no code entered)"),
            (_, l) if l.trim().is_empty() => continue,
            (_, l) => l.trim().to_string(),
        };

        let res = http
            .post(&exchange)
            .json(&serde_json::json!({ "code": code, "state": state }))
            .send()
            .await
            .context("redeeming the sign-in code")?;
        if res.status() == reqwest::StatusCode::BAD_REQUEST {
            eprintln!(
                "{} That code didn't work — check it and try again. Codes expire after\n  \
                 10 minutes; if yours has, re-run `wk login` (Ctrl-C to cancel).",
                style::red("✗"),
            );
            continue;
        }
        let body: serde_json::Value = res
            .error_for_status()
            .context("redeeming the sign-in code")?
            .json()
            .await?;
        return body
            .get("token")
            .and_then(|t| t.as_str())
            .map(str::to_string)
            .context("platform response had no token");
    }
}

fn code_login_url(base_url: &str, state: &str, source: Option<&str>) -> Result<String> {
    let mut url = Url::parse(&format!("{base_url}/cli-login"))
        .with_context(|| format!("invalid base URL {base_url:?}"))?;
    url.query_pairs_mut()
        .append_pair("mode", "code")
        .append_pair("state", state)
        .append_pair("client", CLIENT);
    if let Some(source) = source.filter(|s| !s.is_empty()) {
        url.query_pairs_mut().append_pair("source", source);
    }
    Ok(url.to_string())
}

/// The machine name shown on the consent screen as "from <host>" —
/// same default the loopback handshake uses.
fn hostname() -> Option<String> {
    let out = std::process::Command::new("hostname").output().ok()?;
    let name = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !name.is_empty()).then_some(name)
}

/// True when a browser launched from this process almost certainly
/// can't be seen by the user: we're inside an SSH session, or on a
/// Linux/BSD box with no X11/Wayland display. (`webbrowser::open` would
/// "succeed" there by launching nothing useful, or a TUI browser.)
fn is_remote_session() -> bool {
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    if set("SSH_CONNECTION") || set("SSH_CLIENT") || set("SSH_TTY") {
        return true;
    }
    cfg!(all(unix, not(target_os = "macos"))) && !set("DISPLAY") && !set("WAYLAND_DISPLAY")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_login_url_has_no_callback() {
        let url = code_login_url("https://platform.wavekat.com", "abc123", Some("my box")).unwrap();
        assert_eq!(
            url,
            "https://platform.wavekat.com/cli-login?mode=code&state=abc123&client=wavekat-cli&source=my+box"
        );
    }

    #[test]
    fn code_login_url_omits_empty_source() {
        let url = code_login_url("http://localhost:5173", "s", Some("")).unwrap();
        assert!(!url.contains("source"), "{url}");
        assert!(code_login_url("not a url", "s", None).is_err());
    }
}
