//! `wk version` — print the local CLI version and probe the remote
//! platform's `/api/health`. Unauthenticated; works pre-login.
//!
//! Endpoint resolution (first match wins):
//!   1. `--url` flag
//!   2. `base_url` from `auth.json` if the user has logged in
//!   3. compiled-in default (`login::DEFAULT_BASE_URL`)

use anyhow::{anyhow, Context, Result};
use clap::Args as ClapArgs;
use serde::Deserialize;

use crate::commands::login::DEFAULT_BASE_URL;
use crate::config;
use crate::style;

#[derive(ClapArgs)]
pub struct Args {
    /// Override the platform base URL (defaults to your logged-in
    /// endpoint, or the public platform).
    #[arg(long)]
    url: Option<String>,
    /// Print the result as JSON instead of a formatted block.
    #[arg(long)]
    json: bool,
}

#[derive(Deserialize)]
struct Health {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    commit: Option<String>,
}

const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn run(args: Args) -> Result<()> {
    let base_url = resolve_base_url(args.url.as_deref())?;
    let url = format!("{}/api/health", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .user_agent(concat!("wavekat-cli/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "{} {}: {}",
            status.as_u16(),
            url,
            truncate(&text, 300)
        ));
    }
    let health: Health = serde_json::from_str(&text)
        .with_context(|| format!("decoding /api/health response: {}", truncate(&text, 300)))?;

    if args.json {
        // Re-emit as a stable shape rather than passing the raw body
        // through, so consumers don't accidentally couple to platform
        // field churn.
        let out = serde_json::json!({
            "cli": { "version": CLI_VERSION },
            "api": {
                "ok": health.ok,
                "version": health.version,
                "commit": health.commit,
            },
            "endpoint": base_url,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    let label = |s: &str| style::dim(&format!("{s:<10}"));
    println!("{} {}", label("wk"), style::bold(CLI_VERSION));
    let api_line = match (health.version.as_deref(), health.commit.as_deref()) {
        (Some(v), Some(c)) => format!(
            "{} {}",
            style::bold(v),
            style::dim(&format!("(commit {})", short_sha(c))),
        ),
        (Some(v), None) => style::bold(v).to_string(),
        (None, _) => style::dim("unknown").to_string(),
    };
    println!("{} {api_line}", label("api"));
    println!("{} {}", label("endpoint"), style::dim(&base_url));
    if !health.ok {
        println!(
            "{} {}",
            label("status"),
            style::yellow("api responded but ok=false"),
        );
    }
    Ok(())
}

fn resolve_base_url(flag: Option<&str>) -> Result<String> {
    if let Some(u) = flag {
        return Ok(u.to_string());
    }
    if let Ok(cfg) = config::load() {
        return Ok(cfg.base_url);
    }
    Ok(DEFAULT_BASE_URL.to_string())
}

fn short_sha(s: &str) -> String {
    if s.len() > 7 {
        s[..7].to_string()
    } else {
        s.to_string()
    }
}

fn truncate(s: &str, n: usize) -> &str {
    if s.len() > n {
        &s[..n]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_sha_trims_full_hash() {
        assert_eq!(short_sha("0123456789abcdef"), "0123456");
    }

    #[test]
    fn short_sha_passes_short_input() {
        assert_eq!(short_sha("abc"), "abc");
    }
}
