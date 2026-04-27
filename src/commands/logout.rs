use anyhow::Result;

use crate::config;

pub async fn run() -> Result<()> {
    if config::clear()? {
        println!("Signed out.");
    } else {
        println!("Already signed out.");
    }
    Ok(())
}
