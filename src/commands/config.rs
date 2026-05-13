//! `wk config …` — read and write persisted CLI preferences.
//!
//! Currently a single nested command (`telemetry`) but defined as a
//! subcommand tree so the surface can grow without further refactor.

use anyhow::Result;
use clap::Subcommand;

use crate::config;
use crate::style;

#[derive(Subcommand)]
pub enum Cmd {
    /// Read or change the crash / error reporting setting
    Telemetry(TelemetryArgs),
}

#[derive(clap::Args)]
pub struct TelemetryArgs {
    #[command(subcommand)]
    command: Option<TelemetrySub>,
}

#[derive(Subcommand)]
pub enum TelemetrySub {
    /// Enable crash / error reporting (the default)
    On,
    /// Disable crash / error reporting persistently
    Off,
    /// Print the current setting
    Status,
}

pub async fn run(cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Telemetry(args) => match args.command.unwrap_or(TelemetrySub::Status) {
            TelemetrySub::On => set(true),
            TelemetrySub::Off => set(false),
            TelemetrySub::Status => status(),
        },
    }
}

fn set(enabled: bool) -> Result<()> {
    let mut cfg = config::load_or_default();
    cfg.telemetry = Some(enabled);
    cfg.telemetry_notice_shown = true;
    config::save(&cfg)?;
    if enabled {
        println!("{} crash reporting on", style::green("✓"));
    } else {
        println!("{} crash reporting off", style::green("✓"));
    }
    Ok(())
}

fn status() -> Result<()> {
    let cfg = config::load_or_default();
    let env_override = std::env::var("WK_TELEMETRY").ok();
    let env_disables = matches!(
        env_override.as_deref().map(|s| s.trim().to_ascii_lowercase()),
        Some(ref v) if matches!(v.as_str(), "" | "0" | "false" | "off" | "no")
    );
    let persisted = cfg.telemetry.unwrap_or(true);
    let effective = persisted && !env_disables;

    println!("crash reporting:   {}", on_off(effective));
    println!("  persisted:       {}", on_off(persisted));
    if let Some(v) = env_override {
        println!("  WK_TELEMETRY env: {v:?} ({})", on_off(!env_disables));
    } else {
        println!("  WK_TELEMETRY env: (unset)");
    }
    if !cfg!(feature = "telemetry") {
        println!("  build:            telemetry feature OFF — this binary will not send events");
    }
    Ok(())
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}
