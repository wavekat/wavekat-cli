use anyhow::Result;

use crate::client::Client;
use crate::config;
use crate::style;

pub async fn run() -> Result<()> {
    // Best-effort server-side revoke before we forget the credentials
    // locally. If the platform is unreachable we still clear the file —
    // the user clearly wants to be signed out, and they can revoke from
    // the web UI later.
    if let Ok(client) = Client::from_config() {
        let _ = client.post_empty("/api/auth/cli/tokens/revoke-current").await;
    }
    if config::clear()? {
        println!("{} Signed out.", style::green("✓"));
    } else {
        println!("{}", style::dim("Already signed out."));
    }
    Ok(())
}
