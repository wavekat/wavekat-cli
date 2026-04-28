use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, COOKIE};
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
