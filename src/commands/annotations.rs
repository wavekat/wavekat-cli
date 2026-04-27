use anyhow::Result;
use clap::{Args as ClapArgs, Subcommand};
use serde::Serialize;

use crate::client::Client;

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
    let v: serde_json::Value = client.get_json_query(&path, &query).await?;
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}
