// `wk login` — loopback OAuth flow.
//
// The CLI:
//   1. Binds a TCP listener on 127.0.0.1:<random ephemeral port>.
//   2. Generates a one-shot CSRF state.
//   3. Opens the platform's `/cli-login` page in the user's default
//      browser, with the loopback URL and state as query params.
//   4. Blocks on `accept()` until the platform redirects the browser
//      back to the loopback URL with `?token=…&state=…` (success) or
//      `?error=…&state=…` (cancel/error).
//   5. Verifies the token by calling `/api/me`, persists it, and
//      returns control.
//
// The loopback HTTP server is intentionally hand-rolled (std::net):
// one request, one response, no concurrency, no need to drag in a
// framework. The handler reads at most a small fixed amount per
// connection so a stray local probe can't tie us up.
//
// `--no-browser` falls back to printing the URL for the user to open
// manually — useful on a remote host where no browser is available.
// `--token` skips the dance entirely (e.g. for CI), accepting a
// pre-minted `wkcli_…` token.

use anyhow::{anyhow, bail, Context, Result};
use clap::Args as ClapArgs;
use rand::RngCore;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use crate::client::Client;
use crate::config::{self, AuthConfig};
use crate::style;

const DEFAULT_BASE_URL: &str = "https://platform.wavekat.com";

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

    /// Pre-minted `wkcli_…` bearer token. Skips the browser handshake
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
        None => browser_handshake(&base_url, args.no_browser)?,
    };
    if token.is_empty() {
        bail!("got an empty token from the platform");
    }

    let cfg = AuthConfig {
        base_url,
        token: Some(token),
        session_cookie: None,
    };

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

fn browser_handshake(base_url: &str, no_browser: bool) -> Result<String> {
    // Bind to an ephemeral port on loopback only — never on 0.0.0.0,
    // since anything bound to a non-loopback interface could be reached
    // by another host on the network for the brief window we listen.
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .context("binding loopback listener for the OAuth handshake")?;
    let port = listener.local_addr()?.port();

    let state = random_state();
    let name = client_name();
    let callback = format!("http://127.0.0.1:{port}/callback");

    let auth_url = format!(
        "{base_url}/cli-login?callback={cb}&state={state}&name={name}",
        cb = url::form_urlencoded::byte_serialize(callback.as_bytes()).collect::<String>(),
        state = url::form_urlencoded::byte_serialize(state.as_bytes()).collect::<String>(),
        name = url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>(),
    );

    if no_browser {
        println!("Open this URL in any browser to finish signing in:\n  {auth_url}\n");
    } else {
        println!("Opening {base_url} in your browser to sign in…");
        if let Err(e) = webbrowser::open(&auth_url) {
            eprintln!("(couldn't open the browser automatically: {e})");
            println!("Open this URL manually:\n  {auth_url}\n");
        }
    }
    println!("Waiting for the browser to redirect back (Ctrl-C to cancel)…");

    // 5 minutes is generous — if a user takes longer than that to log in
    // they probably got distracted. Re-running `wk login` is cheap.
    listener
        .set_nonblocking(false)
        .context("listener: set_blocking")?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5 * 60);

    loop {
        if std::time::Instant::now() > deadline {
            bail!("timed out waiting for the browser to complete the login");
        }
        let (stream, _) = listener
            .accept()
            .context("accepting browser callback connection")?;
        match handle_callback(stream, &state) {
            Ok(Some(token)) => return Ok(token),
            Ok(None) => continue, // probe / preflight; keep listening
            Err(e) => {
                // Surface but don't abort — a stray request shouldn't break
                // the real one. (e.g. devtools sending a HEAD probe.)
                eprintln!("(ignored bad callback request: {e})");
                continue;
            }
        }
    }
}

fn handle_callback(mut stream: TcpStream, expected_state: &str) -> Result<Option<String>> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .ok();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .ok();

    // Read just the request line + headers. Cap at 8 KiB — the URL we care
    // about is well under that and we never need the body.
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .context("reading HTTP request line")?;
    // Drain headers (and discard) so the browser sees the connection close
    // cleanly. Bounded to keep a malicious local probe from streaming forever.
    let mut header_bytes = 0usize;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        header_bytes += n;
        if header_bytes > 8192 {
            bail!("request headers too large");
        }
    }

    // Parse "GET /callback?... HTTP/1.1"
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    if method != "GET" {
        respond(&mut stream, 405, "method not allowed", "method not allowed")?;
        return Ok(None);
    }
    if !target.starts_with("/callback") {
        // Browsers fetch /favicon.ico; OS sometimes probes /. Reply 404 and
        // keep listening — only /callback matters.
        respond(&mut stream, 404, "not found", "not found")?;
        return Ok(None);
    }

    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut token: Option<String> = None;
    let mut state: Option<String> = None;
    let mut error: Option<String> = None;
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match k.as_ref() {
            "token" => token = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }

    // Constant-time enough: states are short, equal length on success path.
    if state.as_deref() != Some(expected_state) {
        respond(
            &mut stream,
            400,
            "bad state",
            "<h1>State mismatch</h1><p>Re-run <code>wk login</code> to start over.</p>",
        )?;
        bail!("state mismatch — refusing token");
    }

    if let Some(err) = error {
        respond(
            &mut stream,
            200,
            "OK",
            &format!(
                "<h1>Login cancelled</h1><p>You can close this tab and re-run <code>wk login</code>.</p><p style='color:#888'>reason: {}</p>",
                html_escape(&err),
            ),
        )?;
        bail!("login cancelled in browser ({err})");
    }

    let Some(tok) = token else {
        respond(&mut stream, 400, "missing token", "missing token")?;
        bail!("callback missing token");
    };

    respond(
        &mut stream,
        200,
        "OK",
        "<!doctype html><html><head><meta charset=utf-8><title>WaveKat CLI signed in</title><style>body{font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem;color:#1a1a1a}code{background:#f3f4f6;padding:.1em .3em;border-radius:.25em}</style></head><body><h1>You're signed in.</h1><p>You can close this tab and return to your terminal.</p></body></html>",
    )?;
    Ok(Some(tok))
}

fn respond(stream: &mut TcpStream, status: u16, reason: &str, body: &str) -> Result<()> {
    let body_bytes = body.as_bytes();
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        len = body_bytes.len(),
    );
    stream.write_all(resp.as_bytes())?;
    stream.write_all(body_bytes)?;
    // Drain any unread bytes so the kernel doesn't RST the connection
    // before the browser reads our response.
    let _ = stream.flush();
    let mut sink = [0u8; 64];
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let _ = stream.read(&mut sink);
    Ok(())
}

fn random_state() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    // URL-safe base64 (manual, to avoid pulling in another crate).
    base64url(&bytes)
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHA: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8) | (bytes[i + 2] as u32);
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
        out.push(ALPHA[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let n = (bytes[i] as u32) << 16;
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
    } else if rem == 2 {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8);
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
    }
    out
}

fn client_name() -> String {
    let host = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| hostname().ok())
        .unwrap_or_else(|| "unknown-host".to_string());
    format!("wavekat-cli on {host}")
}

#[cfg(unix)]
fn hostname() -> Result<String> {
    let out = std::process::Command::new("hostname").output()?;
    if !out.status.success() {
        return Err(anyhow!("hostname exited non-zero"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(not(unix))]
fn hostname() -> Result<String> {
    std::env::var("COMPUTERNAME").map_err(|e| anyhow!(e))
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}
