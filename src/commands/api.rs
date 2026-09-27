//! `wk api <path>` — authenticated GET against any platform endpoint,
//! printing the JSON response. The escape hatch for endpoints that have
//! no dedicated `wk` command yet (the `gh api` pattern), and the shared
//! plumbing behind `wk admin`.
//!
//! GET-only on purpose: this exists so people and agents can *read*
//! platform data. Mutations stay behind dedicated commands, where the
//! flags and confirmations are designed per action.

use anyhow::{anyhow, bail, Result};
use clap::Args as ClapArgs;
use serde_json::Value;

use crate::client::Client;

#[derive(ClapArgs)]
pub struct Args {
    /// Endpoint path, e.g. `/api/admin/geo`. The `/api` prefix is
    /// optional (`admin/geo` works too). A literal `?a=b` query string
    /// is kept as-is.
    path: String,
    #[command(flatten)]
    query: QueryArgs,
}

/// Repeatable `-q key=value` query parameters, shared by `wk api` and
/// every `wk admin` command so each one can pass whatever filters the
/// endpoint accepts without the CLI mirroring every server flag.
#[derive(ClapArgs, Default)]
pub struct QueryArgs {
    /// Query parameter as `key=value`; repeat for more
    /// (`-q days=30 -q limit=100`). See `wk admin spec` for what each
    /// endpoint accepts.
    #[arg(short = 'q', long = "query", value_name = "KEY=VALUE")]
    pub(crate) query: Vec<String>,
    /// Accepted for consistency with the other read commands; output is
    /// always JSON.
    #[arg(long, hide = true)]
    json: bool,
}

pub async fn run(args: Args) -> Result<()> {
    let client = Client::from_config()?;
    get_and_print(&client, &normalize_path(&args.path), &args.query).await
}

/// GET `path` with the `-q` parameters and pretty-print the JSON body.
pub async fn get_and_print(client: &Client, path: &str, query: &QueryArgs) -> Result<()> {
    let pairs = parse_query(&query.query)?;
    let body: Value = client.get_json_query(path, &pairs).await?;
    println!("{}", serde_json::to_string_pretty(&body)?);
    Ok(())
}

/// `admin/geo`, `/admin/geo` and `/api/admin/geo` all mean the same
/// endpoint — every platform route lives under `/api`.
pub fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    let with_slash = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    if with_slash == "/api" || with_slash.starts_with("/api/") || with_slash.starts_with("/api?") {
        with_slash
    } else {
        format!("/api{with_slash}")
    }
}

/// Split each `key=value` on the first `=`, so values may themselves
/// contain `=`. An empty value (`key=`) is allowed; a missing `=` or an
/// empty key is a usage error.
pub fn parse_query(raw: &[String]) -> Result<Vec<(String, String)>> {
    raw.iter()
        .map(|kv| {
            let (k, v) = kv
                .split_once('=')
                .ok_or_else(|| anyhow!("query parameter `{kv}` must be KEY=VALUE"))?;
            if k.is_empty() {
                bail!("query parameter `{kv}` has an empty key");
            }
            Ok((k.to_string(), v.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_adds_api_prefix() {
        assert_eq!(normalize_path("admin/geo"), "/api/admin/geo");
        assert_eq!(normalize_path("/admin/geo"), "/api/admin/geo");
        assert_eq!(normalize_path("/api/admin/geo"), "/api/admin/geo");
        assert_eq!(normalize_path("api/me"), "/api/me");
        assert_eq!(normalize_path("  /api/me "), "/api/me");
    }

    #[test]
    fn normalize_does_not_mistake_api_lookalikes() {
        // `/apis` is not under `/api/`, so it gets the prefix.
        assert_eq!(normalize_path("/apis"), "/api/apis");
        assert_eq!(normalize_path("/api?x=1"), "/api?x=1");
    }

    #[test]
    fn parse_query_splits_on_first_equals() {
        let got = parse_query(&["days=30".into(), "q=a=b".into(), "empty=".into()]).unwrap();
        assert_eq!(
            got,
            vec![
                ("days".into(), "30".into()),
                ("q".into(), "a=b".into()),
                ("empty".into(), "".into()),
            ]
        );
    }

    #[test]
    fn parse_query_rejects_malformed() {
        assert!(parse_query(&["days".into()]).is_err());
        assert!(parse_query(&["=30".into()]).is_err());
    }
}
