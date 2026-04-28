use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Persisted credentials. The CLI now authenticates with a bearer token
/// minted via the loopback OAuth flow (see `commands::login`); the older
/// `session_cookie` field is still read so existing installs keep working
/// until the next `wk login`.
#[derive(Serialize, Deserialize, Clone)]
pub struct AuthConfig {
    pub base_url: String,
    /// `wkcli_…` bearer token issued by `POST /api/auth/cli/tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Legacy: raw value of the `wk_session` cookie. Read for back-compat,
    /// not written by new logins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_cookie: Option<String>,
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
