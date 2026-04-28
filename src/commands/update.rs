//! `wk update` — replace the running binary with the latest release.
//!
//! Strategy: download `install.sh` from `/releases/latest` and pipe it
//! to `sh`, with `WK_INSTALL_DIR` pointed at the directory containing
//! the running binary so we replace this exact install rather than
//! some other copy on `$PATH`. Delegating to the same script users run
//! via `curl | sh` keeps target detection, checksum verification, and
//! archive extraction in one tested place instead of duplicating it
//! here in Rust.
//!
//! Linux quirk: GNU coreutils `install` opens the destination with
//! `O_CREAT|O_TRUNC`, which fails with `ETXTBSY` when the destination
//! is the currently running executable. We sidestep that by renaming
//! the running binary aside first; the kernel keeps the running image
//! alive via the inode the process already holds open, so the rest of
//! `wk update` (and any subsequent installer steps) keeps working.

use anyhow::{anyhow, bail, Context, Result};
use clap::Args as ClapArgs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::style;

const REPO: &str = "wavekat/wavekat-cli";
const CURRENT: &str = env!("CARGO_PKG_VERSION");
const INSTALL_SH_URL: &str =
    "https://github.com/wavekat/wavekat-cli/releases/latest/download/install.sh";

#[derive(ClapArgs)]
pub struct Args {
    /// Check whether a newer release exists, but don't install it.
    #[arg(long)]
    check: bool,
    /// Pin a specific tag (e.g. `v0.0.7`) instead of installing the
    /// latest release.
    #[arg(long)]
    version: Option<String>,
}

pub async fn run(args: Args) -> Result<()> {
    if args.check {
        return run_check(args.version.as_deref()).await;
    }
    run_install(args.version.as_deref()).await
}

async fn run_check(pin: Option<&str>) -> Result<()> {
    let target = match pin {
        Some(v) => v.trim_start_matches('v').to_string(),
        None => resolve_latest_tag().await?.trim_start_matches('v').to_string(),
    };
    if target == CURRENT {
        println!("{} wk {CURRENT} is the latest.", style::green("✓"));
    } else {
        println!(
            "{} wk {} → {}",
            style::yellow("update available:"),
            CURRENT,
            style::bold(&target),
        );
        println!("Run `wk update` to install.");
    }
    Ok(())
}

async fn run_install(pin: Option<&str>) -> Result<()> {
    let cur = std::env::current_exe().context("resolving current executable path")?;
    let install_dir = cur
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("current executable has no parent dir: {}", cur.display()))?;

    // Move the running binary aside so the installer can drop a fresh
    // file at the canonical path without tripping ETXTBSY on Linux.
    let aside = cur.with_file_name(format!(
        "{}.old",
        cur.file_name().and_then(|s| s.to_str()).unwrap_or("wk"),
    ));
    let _ = std::fs::remove_file(&aside);
    std::fs::rename(&cur, &aside).with_context(|| {
        format!(
            "renaming {} aside (need write permission to {})",
            cur.display(),
            install_dir.display(),
        )
    })?;

    let outcome = run_installer(&install_dir, pin).await;

    if outcome.is_err() {
        // Restore the prior binary so a failed update doesn't leave
        // the user without a working `wk`. Best-effort.
        let _ = std::fs::rename(&aside, &cur);
    } else {
        let _ = std::fs::remove_file(&aside);
        println!("{} wk updated.", style::green("✓"));
    }
    outcome
}

async fn run_installer(install_dir: &Path, pin: Option<&str>) -> Result<()> {
    eprintln!("{} fetching installer…", style::dim("·"));
    let client = reqwest::Client::builder()
        .user_agent(concat!("wavekat-cli/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let resp = client
        .get(INSTALL_SH_URL)
        .send()
        .await
        .with_context(|| format!("GET {INSTALL_SH_URL}"))?
        .error_for_status()
        .with_context(|| format!("GET {INSTALL_SH_URL}"))?;
    let script = resp.text().await?;

    let mut cmd = Command::new("sh");
    cmd.arg("-s")
        .env("WK_INSTALL_DIR", install_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(v) = pin {
        cmd.env("WK_VERSION", v);
    }
    let mut child = cmd.spawn().context("spawning sh to run installer")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes()).await?;
        stdin.shutdown().await?;
    }
    let status = child.wait().await?;
    if !status.success() {
        bail!("installer exited with status {status}");
    }
    Ok(())
}

async fn resolve_latest_tag() -> Result<String> {
    // /releases/latest 302s to /tag/<tag>; reading the redirect target
    // gives us the latest tag without parsing the JSON API (and without
    // hitting unauthenticated rate limits).
    let url = format!("https://github.com/{REPO}/releases/latest");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("wavekat-cli/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let resp = client
        .head(&url)
        .send()
        .await
        .with_context(|| format!("HEAD {url}"))?;
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow!("no Location header from {url}"))?;
    location
        .rsplit_once("/tag/")
        .map(|(_, t)| t.to_string())
        .ok_or_else(|| anyhow!("could not parse tag from redirect: {location}"))
}
