use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, COOKIE};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::config::{self, AuthConfig};

pub struct Client {
    inner: reqwest::Client,
    base_url: String,
}

impl Client {
    pub fn from_config() -> Result<Self> {
        let cfg = config::load()?;
        Self::new(&cfg)
    }

    pub fn new(cfg: &AuthConfig) -> Result<Self> {
        let mut headers = HeaderMap::new();
        // Prefer the bearer token (new flow). Fall back to the legacy
        // session cookie so existing auth.json files keep working until
        // the user re-runs `wk login`.
        if let Some(token) = cfg.token.as_deref() {
            let value = format!("Bearer {token}");
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&value).context("token contained invalid bytes")?,
            );
        } else if let Some(cookie) = cfg.session_cookie.as_deref() {
            let value = format!("wk_session={cookie}");
            headers.insert(
                COOKIE,
                HeaderValue::from_str(&value).context("session cookie contained invalid bytes")?,
            );
        } else {
            return Err(anyhow!(
                "no credentials in config — run `wk login` to authenticate"
            ));
        }
        let inner = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(concat!("wavekat-cli/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            inner,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
        })
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// Base URL of the connected platform without the path. Used by
    /// commands that print a clickable link in their done message
    /// (e.g. `wk models push` echoes the model details URL).
    pub fn base_url_for_display(&self) -> &str {
        &self.base_url
    }

    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.url(path);
        let resp = self
            .inner
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        decode(url, resp).await
    }

    pub async fn post_empty(&self, path: &str) -> Result<()> {
        let url = self.url(path);
        let resp = self
            .inner
            .post(&url)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let snippet = if text.len() > 500 {
                &text[..500]
            } else {
                &text
            };
            return Err(anyhow!("{} {}: {}", status.as_u16(), url, snippet));
        }
        Ok(())
    }

    /// POST with an empty body and decode the JSON response. Used by
    /// `wk models push` to call `/finalize` (no body, but the server
    /// returns the updated model row).
    pub async fn post_empty_returning_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.url(path);
        let resp = self
            .inner
            .post(&url)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        decode(url, resp).await
    }

    pub async fn get_json_query<T: DeserializeOwned, Q: Serialize + ?Sized>(
        &self,
        path: &str,
        query: &Q,
    ) -> Result<T> {
        let url = self.url(path);
        let resp = self
            .inner
            .get(&url)
            .query(query)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        decode(url, resp).await
    }

    pub async fn post_json<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = self.url(path);
        let resp = self
            .inner
            .post(&url)
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        decode(url, resp).await
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        let url = self.url(path);
        let resp = self
            .inner
            .delete(&url)
            .send()
            .await
            .with_context(|| format!("DELETE {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "{} {}: {}",
                status.as_u16(),
                url,
                truncate(&text, 500)
            ));
        }
        Ok(())
    }

    /// PUT a request body through the platform proxy upload route, with
    /// the configured bearer / cookie auth attached. Used by
    /// `wk models push` when the platform isn't configured with R2
    /// access keys (e.g. local dev) — bytes flow through the Worker
    /// instead of going direct to R2.
    pub async fn put_proxy_bytes(&self, path: &str, body: Vec<u8>) -> Result<()> {
        let url = self.url(path);
        let resp = self
            .inner
            .put(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .send()
            .await
            .with_context(|| format!("PUT {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "{} {}: {}",
                status.as_u16(),
                url,
                truncate(&text, 500)
            ));
        }
        Ok(())
    }

    /// PUT a request body to a presigned R2 URL. The URL embeds SigV4
    /// auth in its query string, so we deliberately use a fresh
    /// `reqwest::Client` without our own headers — adding `Authorization:
    /// Bearer …` would make S3 reject the request.
    pub async fn put_presigned_bytes(presigned_url: &str, body: Vec<u8>) -> Result<()> {
        let resp = reqwest::Client::new()
            .put(presigned_url)
            .body(body)
            .send()
            .await
            .with_context(|| format!("PUT {presigned_url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "{} presigned PUT: {}",
                status.as_u16(),
                truncate(&text, 500)
            ));
        }
        Ok(())
    }

    /// Stream a GET response body to a writer. Returns the number of bytes
    /// written. Used for manifest + clip downloads where the payload is too
    /// big to comfortably hold in memory.
    pub async fn get_stream_to<W: AsyncWriteExt + Unpin>(
        &self,
        path: &str,
        sink: &mut W,
    ) -> Result<u64> {
        let url = self.url(path);
        let resp = self
            .inner
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "{} {}: {}",
                status.as_u16(),
                url,
                truncate(&text, 500)
            ));
        }
        let mut stream = resp.bytes_stream();
        let mut written: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.with_context(|| format!("reading body of {url}"))?;
            sink.write_all(&bytes)
                .await
                .with_context(|| format!("writing chunk from {url}"))?;
            written += bytes.len() as u64;
        }
        sink.flush().await?;
        Ok(written)
    }
}

async fn decode<T: DeserializeOwned>(url: String, resp: reqwest::Response) -> Result<T> {
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!(
            "{} {}: {}",
            status.as_u16(),
            url,
            truncate(&text, 500)
        ));
    }
    serde_json::from_str(&text)
        .with_context(|| format!("decoding response from {url}: {}", truncate(&text, 500)))
}

fn truncate(s: &str, n: usize) -> &str {
    if s.len() > n {
        &s[..n]
    } else {
        s
    }
}
