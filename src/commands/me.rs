use anyhow::Result;
use serde::Deserialize;

use crate::client::Client;

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
    println!("login: {}", me.login);
    println!("id:    {}", me.id);
    println!("name:  {}", me.name.as_deref().unwrap_or("-"));
    println!("email: {}", me.email.as_deref().unwrap_or("-"));
    println!("role:  {}", me.role);
    Ok(())
}
