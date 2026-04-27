use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, COOKIE};
use serde::de::DeserializeOwned;
use serde::Serialize;

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
        let cookie = format!("wk_session={}", cfg.session_cookie);
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&cookie).context("session cookie contained invalid bytes")?,
        );
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
}

async fn decode<T: DeserializeOwned>(url: String, resp: reqwest::Response) -> Result<T> {
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        let snippet = if text.len() > 500 {
            &text[..500]
        } else {
            &text[..]
        };
        return Err(anyhow!("{} {}: {}", status.as_u16(), url, snippet));
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
