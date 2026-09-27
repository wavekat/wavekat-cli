// `wk login` — loopback OAuth flow.
//
// The actual handshake (loopback listener, CSRF state, callback parsing,
// browser-friendly response page) lives in `wavekat-platform-client`.
// This file is the CLI shell around it: argument parsing, deciding
// whether to open the browser, persisting the resulting token, and
// printing a confirmation that includes the user's role.
//
// On a remote host (SSH session, or Linux with no display) — or with
// `--no-browser` — we print the URL instead of opening a browser. The
// browser's final redirect to `127.0.0.1:<port>` then can't reach this
// machine, so the user can paste that redirect URL back into the
// terminal; we pull the token out of it (checking the CSRF state) and
// race that against the loopback listener, whichever lands first.
// `--token` skips the dance entirely (e.g. for CI), accepting a
// pre-minted `wk_…` token.

use std::io::{BufRead, Write};

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

    /// Skip opening the browser; print the URL instead. Implied when
    /// running over SSH or on Linux without a display. Open the URL in any
    /// browser, then paste the URL it redirects to back into the terminal
    /// (or SSH port-forward the loopback port so the redirect lands).
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

    let mut manual = no_browser || is_remote_session();
    if !manual {
        println!("Opening {base_url} in your browser to sign in…");
        if let Err(e) = webbrowser::open(pending.url()) {
            eprintln!("(couldn't open the browser automatically: {e})");
            manual = true;
        }
    }

    if !manual {
        println!("Waiting for the browser to redirect back (Ctrl-C to cancel)…");
        let outcome = pending
            .wait()
            .await
            .context("waiting for the browser callback")?;
        return Ok(outcome.token.as_str().to_string());
    }

    println!(
        "Open this URL in a browser on any machine to sign in:\n\n  {}\n",
        pending.url(),
    );
    println!(
        "After you authorize, the browser redirects to a {} address. If that\n\
         page fails to load (expected over SSH), copy the full URL from the\n\
         address bar and paste it here.\n",
        style::bold("127.0.0.1"),
    );
    print!("Redirect URL: ");
    let _ = std::io::stdout().flush();

    // Read the pasted URL on a plain OS thread, not `spawn_blocking`:
    // the runtime waits for blocking tasks on shutdown, and a stdin read
    // that never completes would hang `wk` after the loopback path wins.
    // Detached threads are simply dropped when the process exits.
    let state = pending.state().to_string();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.lock().read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    if tx.send(line.clone()).is_err() {
                        return;
                    }
                }
            }
        }
    });

    // Same reasoning for the loopback listener: its accept loop runs on
    // a blocking worker until the 5-minute deadline, so if the paste wins
    // it must not be on our runtime. Give it a detached thread + runtime.
    let (wait_tx, wait_rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        let _ = wait_tx.send(rt.block_on(pending.wait()));
    });
    tokio::pin!(wait_rx);
    loop {
        tokio::select! {
            outcome = &mut wait_rx => {
                println!();
                let outcome = outcome
                    .context("loopback listener stopped unexpectedly")?
                    .context("waiting for the browser callback")?;
                return Ok(outcome.token.as_str().to_string());
            }
            Some(line) = rx.recv() => {
                match parse_pasted_redirect(&line, &state) {
                    Ok(Some(token)) => return Ok(token),
                    Ok(None) => {}
                    Err(e) => eprintln!("{} {e}", style::red("✗")),
                }
                print!("Redirect URL: ");
                let _ = std::io::stdout().flush();
            }
        }
    }
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

/// Extract the token from a pasted `http://127.0.0.1:<port>/callback?…`
/// redirect URL. `Ok(None)` for a blank line; `Err` for anything that
/// isn't a valid callback for *this* handshake.
fn parse_pasted_redirect(input: &str, expected_state: &str) -> Result<Option<String>> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    let query = input.split_once('?').map(|(_, q)| q).unwrap_or(input);
    let query = query.split('#').next().unwrap_or("");

    let (mut token, mut state, mut error) = (None, None, None);
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = percent_decode(v);
        match k {
            "token" => token = Some(v),
            "state" => state = Some(v),
            "error" => error = Some(v),
            _ => {}
        }
    }

    if token.is_none() && error.is_none() {
        bail!("that doesn't look like the redirect URL (no `token=` in it) — try again");
    }
    if state.as_deref() != Some(expected_state) {
        bail!("state mismatch — that URL is from a different sign-in attempt");
    }
    if let Some(err) = error {
        bail!("sign-in was cancelled in the browser ({err})");
    }
    Ok(token.filter(|t| !t.is_empty()))
}

/// Decode `application/x-www-form-urlencoded` values (`+` → space,
/// `%XX` → byte). Invalid escapes are passed through verbatim.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => match bytes.get(i + 1..i + 3).and_then(hex_byte) {
                Some(b) => {
                    out.push(b);
                    i += 2;
                }
                None => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_byte(pair: &[u8]) -> Option<u8> {
    u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "http://127.0.0.1:54321/callback?token=wk_abc%2Bdef&state=s-1_x&login=eason";

    #[test]
    fn pasted_redirect_yields_token() {
        let got = parse_pasted_redirect(&format!("  {URL}\n"), "s-1_x").unwrap();
        assert_eq!(got.as_deref(), Some("wk_abc+def"));
    }

    #[test]
    fn pasted_redirect_accepts_bare_query() {
        let got = parse_pasted_redirect("token=wk_1&state=s", "s").unwrap();
        assert_eq!(got.as_deref(), Some("wk_1"));
    }

    #[test]
    fn pasted_redirect_rejects_wrong_state() {
        assert!(parse_pasted_redirect(URL, "other").is_err());
    }

    #[test]
    fn pasted_redirect_surfaces_cancel() {
        let err = parse_pasted_redirect("/callback?error=denied&state=s", "s").unwrap_err();
        assert!(err.to_string().contains("denied"), "{err}");
    }

    #[test]
    fn pasted_redirect_ignores_blank_and_rejects_junk() {
        assert!(parse_pasted_redirect("   \n", "s").unwrap().is_none());
        assert!(parse_pasted_redirect("hello", "s").is_err());
    }

    #[test]
    fn percent_decode_handles_edges() {
        assert_eq!(percent_decode("a%20b+c"), "a b c");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%é"), "%é");
    }
}
