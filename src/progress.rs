// Tiny in-process spinner. We don't pull in `indicatif` for a single
// frame loop — the rest of the CLI is intentionally dep-light, and this
// is ~30 lines of std + tokio that we already depend on.
//
// `with_spinner(label, fut)` races the work future against a 100 ms
// ticker. On each tick we redraw the line on stderr; when the work
// resolves we clear the line and return its value untouched. Disabled
// automatically when stderr isn't a TTY or `NO_COLOR` is set, so piping
// stays clean.

use std::future::Future;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn enabled() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    std::io::stderr().is_terminal()
}

pub async fn with_spinner<F, T>(label: &str, fut: F) -> T
where
    F: Future<Output = T>,
{
    if !enabled() {
        return fut.await;
    }
    tokio::pin!(fut);
    let started = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut i: usize = 0;
    loop {
        tokio::select! {
            v = &mut fut => {
                let mut err = std::io::stderr().lock();
                let _ = write!(err, "\r\x1b[2K");
                let _ = err.flush();
                return v;
            }
            _ = tick.tick() => {
                let secs = started.elapsed().as_secs();
                let frame = FRAMES[i % FRAMES.len()];
                i = i.wrapping_add(1);
                let mut err = std::io::stderr().lock();
                let _ = write!(
                    err,
                    "\r\x1b[2K{frame} {label}  {}:{:02}",
                    secs / 60,
                    secs % 60,
                );
                let _ = err.flush();
            }
        }
    }
}
