use anyhow::Result;
use serde::Deserialize;

use crate::client::Client;
use crate::style;

#[derive(Deserialize)]
struct Me {
    id: i64,
    login: String,
    name: Option<String>,
    email: Option<String>,
    role: String,
}

pub async fn run() -> Result<()> {
    let client = Client::from_config()?;
    let me: Me = client.get_json("/api/me").await?;
    let label = |s: &str| style::dim(&format!("{s:<6}"));
    println!("{} {}", label("login:"), style::bold(&me.login));
    println!("{} {}", label("id:"), me.id);
    println!("{} {}", label("name:"), me.name.as_deref().unwrap_or("-"));
    println!("{} {}", label("email:"), me.email.as_deref().unwrap_or("-"));
    println!("{} {}", label("role:"), style::role(&me.role));
    Ok(())
}
