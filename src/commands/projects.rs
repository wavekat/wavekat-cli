use anyhow::Result;
use clap::{Args as ClapArgs, Subcommand};
use serde::{Deserialize, Serialize};

use crate::client::Client;
use crate::style;

#[derive(Subcommand)]
pub enum Cmd {
    /// List projects you can see (`GET /api/projects`)
    List(ListArgs),
    /// Show a single project (`GET /api/projects/{id}`)
    Show(ShowArgs),
}

#[derive(ClapArgs)]
pub struct ListArgs {
    #[arg(long, default_value_t = 1)]
    page: u32,
    #[arg(long, default_value_t = 20)]
    page_size: u32,
    /// Print raw JSON instead of a table
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs)]
pub struct ShowArgs {
    /// Project id (uuid)
    project_id: String,
    /// Print raw JSON instead of a summary
    #[arg(long)]
    json: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    id: String,
    name: String,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    my_role_in_project: Option<String>,
    #[serde(default)]
    files_count: Option<i64>,
    #[serde(default)]
    annotations_count: Option<i64>,
    #[serde(default)]
    annotations_reviewed_count: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListResponse {
    projects: Vec<Project>,
    page: u32,
    page_size: u32,
    total: u32,
    total_pages: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    page: u32,
    page_size: u32,
}

pub async fn run(cmd: Cmd) -> Result<()> {
    let client = Client::from_config()?;
    match cmd {
        Cmd::List(args) => list(&client, args).await,
        Cmd::Show(args) => show(&client, args).await,
    }
}

async fn list(client: &Client, args: ListArgs) -> Result<()> {
    let query = ListQuery {
        page: args.page,
        page_size: args.page_size,
    };
    if args.json {
        let v: serde_json::Value = client.get_json_query("/api/projects", &query).await?;
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let resp: ListResponse = client.get_json_query("/api/projects", &query).await?;
    if resp.projects.is_empty() {
        println!("No projects.");
        return Ok(());
    }
    println!(
        "{}  {}  {}  {}  {}  {}  {}",
        style::bold(&format!("{:<38}", "ID")),
        style::bold(&format!("{:<22}", "NAME")),
        style::bold(&format!("{:<8}", "ROLE")),
        style::bold(&format!("{:>5}", "FILES")),
        style::bold(&format!("{:>7}", "RECORDS")),
        style::bold(&format!("{:>13}", "REVIEWED")),
        style::bold("UPDATED"),
    );
    for p in &resp.projects {
        // Pad to fixed widths in raw bytes first, then style — ANSI escape
        // codes count as bytes (not columns) inside Rust's `{:<N}`, so
        // styling has to wrap an already-padded cell or columns drift.
        let name = truncate(&p.name, 22);
        let role = p.my_role_in_project.as_deref().unwrap_or("—");
        let files = p
            .files_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".into());
        let records = p
            .annotations_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".into());
        let reviewed = format_reviewed(p.annotations_reviewed_count, p.annotations_count);
        let updated = p.updated_at.as_deref().unwrap_or("—");
        println!(
            "{}  {name:<22}  {}  {}  {}  {}  {}",
            style::dim(&format!("{:<38}", p.id)),
            style::cyan(&format!("{role:<8}")),
            style::dim(&format!("{files:>5}")),
            style::dim(&format!("{records:>7}")),
            style::dim(&format!("{reviewed:>13}")),
            style::dim(updated),
        );
    }
    println!(
        "\n{}",
        style::dim(&format!(
            "Page {}/{} · {} project(s) total · pageSize {}",
            resp.page, resp.total_pages, resp.total, resp.page_size
        )),
    );
    if resp.page < resp.total_pages {
        println!(
            "{} wk projects list --page {}{}",
            style::dim("Next:"),
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

async fn show(client: &Client, args: ShowArgs) -> Result<()> {
    let path = format!("/api/projects/{}", args.project_id);
    let v: serde_json::Value = client.get_json(&path).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("-").to_string();
    let label = |s: &str| style::dim(&format!("{s:<13}"));
    println!("{} {}", label("id:"), style::dim(&s("id")));
    println!("{} {}", label("name:"), style::bold(&s("name")));
    if let Some(desc) = v.get("description").and_then(|x| x.as_str()) {
        if !desc.trim().is_empty() {
            println!("{} {desc}", label("description:"));
        }
    }
    println!("{} {}", label("created:"), s("createdAt"));
    println!("{} {}", label("updated:"), s("updatedAt"));
    let label_set_label = match (
        v.get("activeLabelSetName").and_then(|x| x.as_str()),
        v.get("activeLabelSetId").and_then(|x| x.as_str()),
    ) {
        (Some(name), Some(id)) => Some(format!("{name} ({id})")),
        (Some(name), None) => Some(name.to_string()),
        (None, Some(id)) => Some(id.to_string()),
        (None, None) => None,
    };
    if let Some(ls) = label_set_label {
        println!("{} {}", label("label set:"), ls);
    }
    let role = v
        .get("myRoleInProject")
        .or_else(|| v.get("role"))
        .and_then(|x| x.as_str());
    if let Some(role) = role {
        println!("{} {}", label("your role:"), style::cyan(role));
    }
    if let Some(n) = v.get("filesCount").and_then(|x| x.as_i64()) {
        println!("{} {n}", label("files:"));
    }
    let records = v.get("annotationsCount").and_then(|x| x.as_i64());
    let reviewed = v.get("annotationsReviewedCount").and_then(|x| x.as_i64());
    if let Some(total) = records {
        let suffix = match reviewed {
            Some(r) if total > 0 => {
                let pct = (r as f64 / total as f64 * 100.0).round() as i64;
                format!("  ({r} reviewed, {pct}%)")
            }
            Some(r) => format!("  ({r} reviewed)"),
            None => String::new(),
        };
        println!("{} {total}{suffix}", label("records:"));
    }
    Ok(())
}

/// Render the "reviewed" cell as `<reviewed> (NN%)` when both reviewed
/// and total are known. The total itself lives in the RECORDS column,
/// so we drop it here to keep the column narrow. Falls back to em-dash
/// when nothing's known.
fn format_reviewed(reviewed: Option<i64>, total: Option<i64>) -> String {
    match (reviewed, total) {
        (Some(r), Some(t)) if t > 0 => {
            let pct = (r as f64 / t as f64 * 100.0).round() as i64;
            format!("{r} ({pct}%)")
        }
        (Some(r), Some(_)) => format!("{r}"),
        (Some(r), None) => r.to_string(),
        _ => "—".to_string(),
    }
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
    fn truncate_passes_short_strings() {
        assert_eq!(truncate("hi", 28), "hi");
        assert_eq!(truncate("", 5), "");
    }

    #[test]
    fn format_reviewed_handles_known_pair() {
        assert_eq!(format_reviewed(Some(82), Some(100)), "82 (82%)");
        assert_eq!(format_reviewed(Some(2868), Some(2868)), "2868 (100%)");
        assert_eq!(format_reviewed(Some(0), Some(0)), "0");
        assert_eq!(format_reviewed(None, Some(10)), "—");
        assert_eq!(format_reviewed(Some(5), None), "5");
        assert_eq!(format_reviewed(None, None), "—");
    }

    #[test]
    fn truncate_clips_long_strings_with_ellipsis() {
        let out = truncate("abcdefghij", 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('…'));
        assert_eq!(out, "abcd…");
    }
}
