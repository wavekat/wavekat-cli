use anyhow::Result;
use clap::{Args as ClapArgs, Subcommand};
use serde::{Deserialize, Serialize};

use crate::client::Client;

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
    created_at: String,
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
    println!("{:<38}  {:<28}  CREATED", "ID", "NAME");
    for p in &resp.projects {
        println!(
            "{:<38}  {:<28}  {}",
            p.id,
            truncate(&p.name, 28),
            p.created_at
        );
    }
    println!(
        "\nPage {}/{} · {} project(s) total · pageSize {}",
        resp.page, resp.total_pages, resp.total, resp.page_size
    );
    if resp.page < resp.total_pages {
        println!(
            "Next: wk projects list --page {}{}",
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
    let s = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or("-")
            .to_string()
    };
    println!("id:          {}", s("id"));
    println!("name:        {}", s("name"));
    if let Some(desc) = v.get("description").and_then(|x| x.as_str()) {
        if !desc.trim().is_empty() {
            println!("description: {desc}");
        }
    }
    println!("created:     {}", s("createdAt"));
    println!("updated:     {}", s("updatedAt"));
    if let Some(ls) = v.get("activeLabelSetId").and_then(|x| x.as_str()) {
        println!("label set:   {ls}");
    }
    if let Some(role) = v.get("role").and_then(|x| x.as_str()) {
        println!("your role:   {role}");
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
