// Thin wrapper around `wavekat_platform_client::Client` so the CLI's
// command modules keep their familiar `Client::from_config()` /
// `client.get_json(…)` shape while the actual HTTP plumbing lives in
// the shared crate. Adding methods here should be rare — when a method
// is useful to a second consumer it belongs in the platform-client
// crate, not in this wrapper.

use anyhow::{anyhow, Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use wavekat_platform_client::{Client as Inner, Token};

use crate::config::{self, AuthConfig};

pub struct Client {
    inner: Inner,
}

impl Client {
    pub fn from_config() -> Result<Self> {
        let cfg = config::load()?;
        Self::new(&cfg)
    }

    pub fn new(cfg: &AuthConfig) -> Result<Self> {
        let token = cfg.token.as_deref().ok_or_else(|| {
            // The session-cookie bridge from the pre-bearer auth scheme
            // was dropped when this crate started using
            // `wavekat-platform-client`; tell the user how to get back
            // to a working state instead of silently 401-ing.
            if cfg.session_cookie.is_some() {
                anyhow!("legacy session-cookie auth is no longer supported — run `wk login` to mint a wk_ token")
            } else {
                anyhow!("no credentials in config — run `wk login` to authenticate")
            }
        })?;
        let inner = Inner::new(cfg.base_url.as_str(), Token::new(token))
            .context("building HTTP client")?;
        Ok(Self { inner })
    }

    /// Base URL of the connected platform without the path. Used by
    /// commands that print a clickable link in their done message
    /// (e.g. `wk models push` echoes the model details URL).
    pub fn base_url_for_display(&self) -> &str {
        self.inner.base_url()
    }

    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Ok(self.inner.get_json(path).await?)
    }

    pub async fn post_empty(&self, path: &str) -> Result<()> {
        Ok(self.inner.post_empty(path).await?)
    }

    /// POST with an empty body and decode the JSON response. Used by
    /// `wk models push` to call `/finalize` (no body, but the server
    /// returns the updated model row).
    pub async fn post_empty_returning_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Ok(self.inner.post_empty_returning_json(path).await?)
    }

    pub async fn get_json_query<T: DeserializeOwned, Q: Serialize + ?Sized>(
        &self,
        path: &str,
        query: &Q,
    ) -> Result<T> {
        Ok(self.inner.get_json_query(path, query).await?)
    }

    pub async fn post_json<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        Ok(self.inner.post_json(path, body).await?)
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        Ok(self.inner.delete(path).await?)
    }

    /// PUT a request body through the platform proxy upload route, with
    /// the configured bearer auth attached. Used by `wk models push`
    /// when the platform isn't configured with R2 access keys (e.g.
    /// local dev) — bytes flow through the Worker instead of going
    /// direct to R2.
    pub async fn put_proxy_bytes(&self, path: &str, body: Vec<u8>) -> Result<()> {
        Ok(self.inner.put_proxy_bytes(path, body).await?)
    }

    /// PUT a request body to a presigned R2 URL. Implemented as an
    /// associated function (not a method) because the presigned URL
    /// embeds its own SigV4 auth — adding our bearer header would make
    /// S3/R2 reject the request.
    pub async fn put_presigned_bytes(presigned_url: &str, body: Vec<u8>) -> Result<()> {
        Ok(Inner::put_presigned_bytes(presigned_url, body).await?)
    }

    /// Stream a GET response body to a writer. Returns the number of bytes
    /// written. Used for manifest + clip downloads where the payload is too
    /// big to comfortably hold in memory.
    pub async fn get_stream_to<W: AsyncWriteExt + Unpin>(
        &self,
        path: &str,
        sink: &mut W,
    ) -> Result<u64> {
        Ok(self.inner.get_stream_to(path, sink).await?)
    }
}
