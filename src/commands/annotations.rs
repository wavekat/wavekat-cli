use anyhow::Result;
use clap::{Args as ClapArgs, Subcommand};
use serde::{Deserialize, Serialize};

use crate::client::Client;
use crate::style;

#[derive(Subcommand)]
pub enum Cmd {
    /// List annotations in a project (`GET /api/projects/{id}/annotations`)
    List(ListArgs),
}

#[derive(ClapArgs)]
pub struct ListArgs {
    /// Project id (uuid)
    project_id: String,
    #[arg(long, default_value_t = 1)]
    page: u32,
    #[arg(long, default_value_t = 20)]
    page_size: u32,
    /// Filter by label key (e.g. `end_of_turn`)
    #[arg(long)]
    label: Option<String>,
    /// Filter by review status: `approved` | `rejected` | `needs_fix` | `unreviewed`
    #[arg(long)]
    review_status: Option<String>,
    /// Filter to a single source file id
    #[arg(long)]
    file_id: Option<String>,
    /// Filter to a labeller's user id
    #[arg(long)]
    created_by: Option<i64>,
    /// Print raw JSON instead of a table
    #[arg(long)]
    json: bool,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    page: u32,
    page_size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    label_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    review_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_by: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Annotation {
    id: String,
    file_name: Option<String>,
    label_key: String,
    label_value: i64,
    start_sec: f64,
    end_sec: f64,
    review_status: Option<String>,
    asr_text: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListResponse {
    annotations: Vec<Annotation>,
    page: u32,
    page_size: u32,
    total: u32,
    total_pages: u32,
}

pub async fn run(cmd: Cmd) -> Result<()> {
    let client = Client::from_config()?;
    match cmd {
        Cmd::List(args) => list(&client, args).await,
    }
}

async fn list(client: &Client, args: ListArgs) -> Result<()> {
    let path = format!("/api/projects/{}/annotations", args.project_id);
    let query = ListQuery {
        page: args.page,
        page_size: args.page_size,
        label_key: args.label,
        review_status: args.review_status,
        file_id: args.file_id,
        created_by: args.created_by,
    };
    if args.json {
        let v: serde_json::Value = client.get_json_query(&path, &query).await?;
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let resp: ListResponse = client.get_json_query(&path, &query).await?;
    if resp.annotations.is_empty() {
        println!("No annotations.");
        return Ok(());
    }
    println!(
        "{}  {}  {}  {}  {}",
        style::bold(&format!("{:<10}", "ID")),
        style::bold(&format!("{:<24}", "FILE")),
        style::bold(&format!("{:<18}", "LABEL")),
        style::bold(&format!("{:<16}", "RANGE")),
        style::bold("REVIEW"),
    );
    for a in &resp.annotations {
        let id_short = a.id.get(..8).unwrap_or(&a.id);
        let file = truncate(a.file_name.as_deref().unwrap_or("-"), 24);
        let label_text = truncate(&format!("{}={}", a.label_key, a.label_value), 18);
        let range = format!("{:.1}–{:.1}s", a.start_sec, a.end_sec);
        // Pad in raw bytes first, then style — ANSI codes don't count toward
        // the visible width, so styling has to wrap an already-padded cell.
        println!(
            "{}  {file:<24}  {}  {}  {}",
            style::dim(&format!("{id_short:<10}")),
            style::cyan(&format!("{label_text:<18}")),
            style::dim(&format!("{range:<16}")),
            style::review(a.review_status.as_deref()),
        );
        if let Some(text) = a.asr_text.as_deref() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                println!("            {}", style::dim(&truncate_width(trimmed, 80)));
            }
        }
    }
    println!(
        "\n{}",
        style::dim(&format!(
            "Page {}/{} · {} annotation(s) total · pageSize {}",
            resp.page, resp.total_pages, resp.total, resp.page_size
        )),
    );
    if resp.page < resp.total_pages {
        println!(
            "{} wk annotations list {} --page {}{}",
            style::dim("Next:"),
            args.project_id,
            resp.page + 1,
            if resp.page_size != 20 {
                format!(" --page-size {}", resp.page_size)
            } else {
                String::new()
            },
        );
    }
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

/// Truncate by *visible* terminal width (CJK chars count as 2). Stops as
/// soon as appending the next char would push us over `cols - 1`, leaving
/// room for the trailing ellipsis. Falls back to no-op when the input
/// already fits.
fn truncate_width(s: &str, cols: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let mut total = 0usize;
    for c in s.chars() {
        total += c.width().unwrap_or(0);
        if total > cols {
            let budget = cols.saturating_sub(1);
            let mut used = 0usize;
            let mut out = String::new();
            for c in s.chars() {
                let w = c.width().unwrap_or(0);
                if used + w > budget {
                    break;
                }
                out.push(c);
                used += w;
            }
            out.push('…');
            return out;
        }
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn truncate_passes_short_strings() {
        assert_eq!(truncate("hi", 5), "hi");
        assert_eq!(truncate("", 5), "");
        // Boundary: exactly n chars must not be truncated.
        assert_eq!(truncate("abcde", 5), "abcde");
    }

    #[test]
    fn truncate_clips_long_strings_with_ellipsis() {
        let out = truncate("abcdefghij", 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('…'));
        assert_eq!(out, "abcd…");
    }

    #[test]
    fn truncate_counts_chars_not_bytes() {
        // Multi-byte chars should be counted once, not by byte length.
        let out = truncate("日本語テキスト", 4);
        assert_eq!(out.chars().count(), 4);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_width_passes_when_fits() {
        assert_eq!(truncate_width("hello", 80), "hello");
    }

    #[test]
    fn truncate_width_respects_visible_columns() {
        let out = truncate_width("abcdefghij", 5);
        assert!(out.ends_with('…'));
        assert!(out.width() <= 5);
        assert_eq!(out, "abcd…");
    }

    #[test]
    fn truncate_width_treats_cjk_as_double_width() {
        // Each CJK char is width 2; budget of 5 fits at most two chars
        // plus the ellipsis (2 + 2 + 1 = 5).
        let out = truncate_width("日本語テキスト", 5);
        assert!(out.ends_with('…'));
        assert!(out.width() <= 5);
    }
}
