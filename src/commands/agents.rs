//! `wk agents` — print the bundled `AGENTS.md` integration guide for
//! AI agents. The file is embedded at build time so the running binary
//! is the authoritative source for whichever release the user installed.

use anyhow::Result;

const AGENTS_MD: &str = include_str!("../../AGENTS.md");

pub async fn run() -> Result<()> {
    print!("{AGENTS_MD}");
    if !AGENTS_MD.ends_with('\n') {
        println!();
    }
    Ok(())
}
