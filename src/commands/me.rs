use anyhow::Result;
use clap::Args as ClapArgs;
use serde::Deserialize;

use crate::client::Client;
use crate::config;
use crate::style;

#[derive(ClapArgs)]
pub struct Args {
    /// Print raw JSON instead of a summary
    #[arg(long)]
    json: bool,
}

#[derive(Deserialize)]
struct Me {
    id: String,
    login: String,
    name: Option<String>,
    email: Option<String>,
    role: String,
}

pub async fn run(args: Args) -> Result<()> {
    let client = Client::from_config()?;
    let v: serde_json::Value = client.get_json("/api/me").await?;
    remember_account(&v);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let me: Me = serde_json::from_value(v)?;
    let label = |s: &str| style::dim(&format!("{s:<6}"));
    println!("{} {}", label("login:"), style::bold(&me.login));
    println!("{} {}", label("id:"), me.id);
    println!("{} {}", label("name:"), me.name.as_deref().unwrap_or("-"));
    println!("{} {}", label("email:"), me.email.as_deref().unwrap_or("-"));
    println!("{} {}", label("role:"), style::role(&me.role));
    Ok(())
}

/// Keep the cached role and modules current, so a role change or a new
/// module grant shows up in `wk --help` without a fresh `wk login`.
/// Best-effort: a failed write never fails `wk me`.
fn remember_account(me: &serde_json::Value) {
    let Ok(mut cfg) = config::load() else {
        return;
    };
    if config::remember_account(&mut cfg, me) {
        let _ = config::save(&cfg);
    }
}
