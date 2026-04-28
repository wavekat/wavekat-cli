//! `wk update` — replace the running binary with the latest release.
//!
//! Strategy: resolve the target tag here (so we can tell the user a
//! clear `vA → vB` line, and short-circuit the no-op case where we're
//! already on the latest), then pipe `install.sh` to `sh` with
//! `WK_VERSION` pinned to that exact tag and `WK_INSTALL_DIR` pointed
//! at the directory containing the running binary. Pinning matters:
//! `install.sh` re-resolves `/releases/latest` on its own, and if a
//! release is mid-promotion at GitHub the two redirects can disagree.
//!
//! Delegating extraction + checksum verification + arch detection to
//! the same `install.sh` users run via `curl | sh` keeps that logic in
//! one tested place instead of duplicating it here in Rust.
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
    /// Reinstall even if the resolved target version matches the
    /// running binary. Useful for re-running `wk update` after a
    /// broken install or to re-pin a specific tag.
    #[arg(long)]
    force: bool,
}

pub async fn run(args: Args) -> Result<()> {
    if args.check {
        return run_check(args.version.as_deref()).await;
    }
    run_install(args.version.as_deref(), args.force).await
}

async fn run_check(pin: Option<&str>) -> Result<()> {
    let target = resolve_target_version(pin).await?;
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

async fn run_install(pin: Option<&str>, force: bool) -> Result<()> {
    // Resolve the target tag up front so the user sees a clear before
    // / after line, and so we can short-circuit the silent no-op case
    // where /releases/latest is still pointing at the previous version
    // (e.g. mid release-plz promotion). Without this, `wk update`
    // would call install.sh, install.sh would resolve the same tag
    // we're already on, and the user would see install.sh's
    // "Installing wk vX" message and assume something newer arrived.
    let target_v = resolve_target_version(pin).await?;
    if target_v == CURRENT && !force {
        println!(
            "{} wk {CURRENT} is the latest — nothing to do. \
             Pass `--force` to reinstall.",
            style::green("✓"),
        );
        return Ok(());
    }
    eprintln!(
        "{} updating wk {} → {}",
        style::dim("·"),
        CURRENT,
        style::bold(&target_v),
    );

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

    // Pin install.sh to the exact tag we resolved so it doesn't
    // re-resolve /releases/latest and potentially pick a different
    // version than the one we just told the user about.
    let pinned = with_v_prefix(&target_v);
    let outcome = run_installer(&install_dir, &pinned).await;

    if outcome.is_err() {
        // Restore the prior binary so a failed update doesn't leave
        // the user without a working `wk`. Best-effort.
        let _ = std::fs::rename(&aside, &cur);
    } else {
        let _ = std::fs::remove_file(&aside);
        println!(
            "{} wk updated to {}.",
            style::green("✓"),
            style::bold(&target_v)
        );
    }
    outcome
}

async fn run_installer(install_dir: &Path, pinned_tag: &str) -> Result<()> {
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
        .env("WK_VERSION", pinned_tag)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
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

async fn resolve_target_version(pin: Option<&str>) -> Result<String> {
    Ok(match pin {
        Some(v) => v.trim_start_matches('v').to_string(),
        None => resolve_latest_tag()
            .await?
            .trim_start_matches('v')
            .to_string(),
    })
}

fn with_v_prefix(version: &str) -> String {
    if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{version}")
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_v_prefix_adds_when_missing() {
        assert_eq!(with_v_prefix("0.0.7"), "v0.0.7");
    }

    #[test]
    fn with_v_prefix_idempotent() {
        assert_eq!(with_v_prefix("v0.0.7"), "v0.0.7");
    }
}
