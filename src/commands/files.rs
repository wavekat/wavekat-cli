//! `wk files …` — list project files, manage the test-set reservation
//! flag (docs/08-test-set-reservation.md).
//!
//! Reservation maps onto two endpoints:
//!   POST   /api/files/{id}/test-reservation     (idempotent)
//!   DELETE /api/files/{id}/test-reservation     (idempotent)
//! Both are owner/root-gated server-side; the CLI just reflects whatever
//! the API returns.

use anyhow::Result;
use clap::{Args as ClapArgs, Subcommand};
use serde::{Deserialize, Serialize};

use crate::client::Client;
use crate::style;

#[derive(Subcommand)]
pub enum Cmd {
    /// List files in a project (`GET /api/projects/{id}/files`)
    List(ListArgs),
    /// Mark one or more files as part of the held-out test set
    /// (`POST /api/files/{id}/test-reservation`)
    Reserve(ReserveArgs),
    /// Clear the held-out reservation on one or more files
    /// (`DELETE /api/files/{id}/test-reservation`)
    Unreserve(UnreserveArgs),
    /// Project-level reservation summary
    /// (`GET /api/projects/{id}/test-reservation-summary`)
    Summary(SummaryArgs),
}

#[derive(ClapArgs)]
pub struct ListArgs {
    /// Project id (uuid)
    project_id: String,
    #[arg(long, default_value_t = 1)]
    page: u32,
    #[arg(long, default_value_t = 20)]
    page_size: u32,
    /// Case-insensitive substring filter against file name.
    #[arg(long)]
    q: Option<String>,
    /// Restrict to reserved (`true`) or non-reserved (`false`) files.
    /// Omit to return both.
    #[arg(long)]
    test_reserved: Option<bool>,
    /// Print raw JSON instead of a table
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs)]
pub struct ReserveArgs {
    /// One or more file ids (uuid)
    #[arg(required = true)]
    file_ids: Vec<String>,
    /// Print raw JSON instead of a per-file status line
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs)]
pub struct UnreserveArgs {
    /// One or more file ids (uuid)
    #[arg(required = true)]
    file_ids: Vec<String>,
    /// Print raw JSON instead of a per-file status line
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs)]
pub struct SummaryArgs {
    /// Project id (uuid)
    project_id: String,
    /// Print raw JSON instead of a one-line summary
    #[arg(long)]
    json: bool,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct ListQuery<'a> {
    page: u32,
    page_size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    q: Option<&'a str>,
    /// API expects the literal strings `"true"` / `"false"` — its zod
    /// schema is an enum, not a coerced bool, so we ship them as-is.
    #[serde(skip_serializing_if = "Option::is_none")]
    test_reserved: Option<&'static str>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileRow {
    id: String,
    name: String,
    duration_sec: f64,
    test_reserved_at: Option<String>,
    #[serde(default)]
    annotation_count: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListResponse {
    files: Vec<FileRow>,
    page: u32,
    page_size: u32,
    total: u32,
    total_pages: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    file_count: i64,
    annotation_count: i64,
    labelled_seconds: f64,
}

pub async fn run(cmd: Cmd) -> Result<()> {
    let client = Client::from_config()?;
    match cmd {
        Cmd::List(args) => list(&client, args).await,
        Cmd::Reserve(args) => reserve(&client, args).await,
        Cmd::Unreserve(args) => unreserve(&client, args).await,
        Cmd::Summary(args) => summary(&client, args).await,
    }
}

async fn list(client: &Client, args: ListArgs) -> Result<()> {
    let path = format!("/api/projects/{}/files", args.project_id);
    let query = ListQuery {
        page: args.page,
        page_size: args.page_size,
        q: args.q.as_deref(),
        test_reserved: match args.test_reserved {
            Some(true) => Some("true"),
            Some(false) => Some("false"),
            None => None,
        },
    };
    if args.json {
        let v: serde_json::Value = client.get_json_query(&path, &query).await?;
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let resp: ListResponse = client.get_json_query(&path, &query).await?;
    if resp.files.is_empty() {
        println!("No files.");
        return Ok(());
    }
    println!(
        "{}  {}  {}  {}  {}",
        style::bold(&format!("{:<38}", "ID")),
        style::bold(&format!("{:<32}", "NAME")),
        style::bold(&format!("{:<10}", "DURATION")),
        style::bold(&format!("{:<8}", "ANNOTS")),
        style::bold("TEST"),
    );
    for f in &resp.files {
        let name = truncate(&f.name, 32);
        let dur = format!("{:.1}s", f.duration_sec);
        let test_cell = if f.test_reserved_at.is_some() {
            style::yellow("reserved")
        } else {
            style::dim("—")
        };
        println!(
            "{}  {name:<32}  {}  {}  {}",
            style::dim(&format!("{:<38}", f.id)),
            style::dim(&format!("{dur:<10}")),
            style::dim(&format!("{:<8}", f.annotation_count)),
            test_cell,
        );
    }
    println!(
        "\n{}",
        style::dim(&format!(
            "Page {}/{} · {} file(s) total · pageSize {}",
            resp.page, resp.total_pages, resp.total, resp.page_size
        )),
    );
    if resp.page < resp.total_pages {
        let mut hint = format!("wk files list {} --page {}", args.project_id, resp.page + 1);
        if resp.page_size != 20 {
            hint.push_str(&format!(" --page-size {}", resp.page_size));
        }
        if let Some(q) = args.q.as_deref() {
            hint.push_str(&format!(" --q {q:?}"));
        }
        if let Some(b) = args.test_reserved {
            hint.push_str(&format!(" --test-reserved {b}"));
        }
        println!("{} {}", style::dim("Next:"), hint);
    }
    Ok(())
}

async fn reserve(client: &Client, args: ReserveArgs) -> Result<()> {
    let mut json_out: Vec<serde_json::Value> = Vec::with_capacity(args.file_ids.len());
    let mut had_error = false;
    for id in &args.file_ids {
        let path = format!("/api/files/{}/test-reservation", id);
        match client
            .post_json::<serde_json::Value, _>(&path, &serde_json::json!({}))
            .await
        {
            Ok(v) => {
                if args.json {
                    json_out.push(v);
                } else {
                    println!("{} {}", style::bold("Reserved"), style::dim(id));
                }
            }
            Err(e) => {
                had_error = true;
                if args.json {
                    json_out.push(serde_json::json!({"id": id, "error": e.to_string()}));
                } else {
                    eprintln!("{} {} ({})", style::red("FAILED"), style::dim(id), e);
                }
            }
        }
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&json_out)?);
    }
    if had_error {
        Err(anyhow::anyhow!("one or more reservations failed"))
    } else {
        Ok(())
    }
}

async fn unreserve(client: &Client, args: UnreserveArgs) -> Result<()> {
    let mut json_out: Vec<serde_json::Value> = Vec::with_capacity(args.file_ids.len());
    let mut had_error = false;
    for id in &args.file_ids {
        let path = format!("/api/files/{}/test-reservation", id);
        match client.delete(&path).await {
            Ok(()) => {
                if args.json {
                    json_out.push(serde_json::json!({"id": id, "ok": true}));
                } else {
                    println!("{} {}", style::bold("Unreserved"), style::dim(id));
                }
            }
            Err(e) => {
                had_error = true;
                if args.json {
                    json_out.push(serde_json::json!({"id": id, "error": e.to_string()}));
                } else {
                    eprintln!("{} {} ({})", style::red("FAILED"), style::dim(id), e);
                }
            }
        }
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&json_out)?);
    }
    if had_error {
        Err(anyhow::anyhow!("one or more unreservations failed"))
    } else {
        Ok(())
    }
}

async fn summary(client: &Client, args: SummaryArgs) -> Result<()> {
    let path = format!("/api/projects/{}/test-reservation-summary", args.project_id);
    if args.json {
        let v: serde_json::Value = client.get_json(&path).await?;
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let s: Summary = client.get_json(&path).await?;
    println!(
        "{} files: {}, annotations: {}, labelled: {:.1}s",
        style::bold("Reserved"),
        s.file_count,
        s.annotation_count,
        s.labelled_seconds,
    );
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_query_serializes_test_reserved() {
        let q = ListQuery {
            page: 1,
            page_size: 20,
            q: None,
            test_reserved: Some("true"),
        };
        // We round-trip through serde_json since the wire format is the
        // same camelCase for both query string and JSON; this exercises
        // the rename without pulling serde_urlencoded as a direct dep.
        let v = serde_json::to_value(&q).unwrap();
        assert_eq!(v.get("testReserved").and_then(|x| x.as_str()), Some("true"));
    }

    #[test]
    fn list_query_omits_test_reserved_when_none() {
        let q = ListQuery {
            page: 1,
            page_size: 20,
            q: None,
            test_reserved: None,
        };
        let v = serde_json::to_value(&q).unwrap();
        assert!(v.get("testReserved").is_none());
    }

    #[test]
    fn truncate_handles_unicode() {
        assert_eq!(truncate("日本語テキスト", 4).chars().count(), 4);
    }
}
