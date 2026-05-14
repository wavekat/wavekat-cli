use anyhow::{Context, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Persisted credentials. The CLI now authenticates with a bearer token
/// minted via the loopback OAuth flow (see `commands::login`); the older
/// `session_cookie` field is still read so existing installs keep working
/// until the next `wk login`.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct AuthConfig {
    #[serde(default)]
    pub base_url: String,
    /// `wk_…` bearer token issued by `POST /api/auth/cli/tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Legacy: raw value of the `wk_session` cookie. Read for back-compat,
    /// not written by new logins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_cookie: Option<String>,
    /// Anonymous per-machine identifier used by crash reporting to
    /// distinguish "this is the same install hitting the error 10
    /// times" from "10 different installs each hit it once." Generated
    /// the first time we need one and never sent in clear; see
    /// docs/01-crash-and-error-reporting.md.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_id: Option<String>,
    /// Crash / error reporting toggle. `None` = use default (on);
    /// `Some(false)` = the user explicitly opted out via
    /// `wk config telemetry off` or the first-run notice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<bool>,
    /// Has the first-run telemetry notice been shown? Persisted so we
    /// only print it once per install, not on every command.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub telemetry_notice_shown: bool,
}

fn config_dir() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("could not resolve user config directory")?
        .join("wavekat");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

pub fn auth_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("auth.json"))
}

pub fn load() -> Result<AuthConfig> {
    let path = auth_path()?;
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "not signed in — run `wk login` first (looked at {})",
            path.display()
        )
    })?;
    let cfg: AuthConfig =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    Ok(cfg)
}

/// Read the config without erroring on absence or parse failure.
/// Used by telemetry init, which has to run before the user has ever
/// logged in. Returns `Default::default()` on any failure so callers
/// can still consult `telemetry` / `install_id` fields uniformly.
pub fn load_or_default() -> AuthConfig {
    let Ok(path) = auth_path() else {
        return AuthConfig::default();
    };
    let Ok(bytes) = fs::read(&path) else {
        return AuthConfig::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Generate a fresh install identifier. Uses the same `rand` crate the
/// rest of the CLI already pulls; the format is a UUIDv4 string but
/// we don't depend on the `uuid` crate just for this.
#[cfg_attr(not(feature = "telemetry"), allow(dead_code))]
pub fn new_install_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    // RFC 4122 §4.4 — set the version (4) and variant (10xx) bits.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}

/// Read the persisted `install_id`, generating + persisting one if
/// missing. Silently no-ops on persistence failure (e.g. read-only
/// home dir) — the install_id is best-effort and we never let it
/// block a real command.
#[cfg_attr(not(feature = "telemetry"), allow(dead_code))]
pub fn ensure_install_id() -> Option<String> {
    let mut cfg = load_or_default();
    if let Some(id) = cfg.install_id.as_deref() {
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    let id = new_install_id();
    cfg.install_id = Some(id.clone());
    // Persistence failure is non-fatal — we just regenerate next run.
    let _ = save(&cfg);
    Some(id)
}

pub fn save(cfg: &AuthConfig) -> Result<()> {
    let path = auth_path()?;
    let bytes = serde_json::to_vec_pretty(cfg)?;
    fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 600 {}", path.display()))?;
    }
    Ok(())
}

pub fn clear() -> Result<bool> {
    let path = auth_path()?;
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        Ok(true)
    } else {
        Ok(false)
    }
}
