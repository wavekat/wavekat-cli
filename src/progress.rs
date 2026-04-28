// Tiny in-process spinner + progress bar. We don't pull in `indicatif`
// for a single frame loop — the rest of the CLI is intentionally
// dep-light, and this is a few dozen lines of std + tokio that we
// already depend on.
//
// `with_spinner(label, fut)` races the work future against a 100 ms
// ticker and returns `(value, elapsed)` so the caller can roll the
// duration into their own done line.
//
// `ProgressBar` is the same idea for known-total work: a background
// task ticks at 100 ms and redraws `[████░░] cur/total · M:SS`, while
// callers `inc()` from any task as units complete. The atomic counter
// keeps the bar safe under `buffer_unordered` concurrency without a
// mutex. On finish the live line is cleared and the elapsed `Duration`
// is returned; the caller decides whether/how to surface the time.
//
// Both are disabled automatically when stderr isn't a TTY or
// `NO_COLOR` is set, so piping stays clean.

use std::future::Future;
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BAR_WIDTH: usize = 20;

fn enabled() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    std::io::stderr().is_terminal()
}

pub async fn with_spinner<F, T>(label: &str, fut: F) -> (T, Duration)
where
    F: Future<Output = T>,
{
    let started = Instant::now();
    if !enabled() {
        let v = fut.await;
        return (v, started.elapsed());
    }
    tokio::pin!(fut);
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut i: usize = 0;
    loop {
        tokio::select! {
            v = &mut fut => {
                let mut err = std::io::stderr().lock();
                let _ = write!(err, "\r\x1b[2K");
                let _ = err.flush();
                return (v, started.elapsed());
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

pub struct ProgressBar {
    state: Arc<ProgressState>,
    task: Option<tokio::task::JoinHandle<()>>,
    enabled: bool,
}

struct ProgressState {
    label: String,
    total: u64,
    current: AtomicU64,
    started: Instant,
}

impl ProgressBar {
    pub fn new(label: impl Into<String>, total: u64) -> Self {
        let enabled = enabled();
        let state = Arc::new(ProgressState {
            label: label.into(),
            total,
            current: AtomicU64::new(0),
            started: Instant::now(),
        });
        let task = if enabled {
            let s = state.clone();
            Some(tokio::spawn(async move {
                let started = s.started;
                let mut tick = tokio::time::interval(Duration::from_millis(100));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut i: usize = 0;
                loop {
                    tick.tick().await;
                    let cur = s.current.load(Ordering::Relaxed);
                    let secs = started.elapsed().as_secs();
                    let frame = FRAMES[i % FRAMES.len()];
                    i = i.wrapping_add(1);
                    let bar = render_bar(cur, s.total, BAR_WIDTH);
                    let mut err = std::io::stderr().lock();
                    let _ = write!(
                        err,
                        "\r\x1b[2K{frame} {label}  [{bar}]  {cur}/{total} · {m}:{ss:02}",
                        label = s.label,
                        cur = cur,
                        total = s.total,
                        m = secs / 60,
                        ss = secs % 60,
                    );
                    let _ = err.flush();
                }
            }))
        } else {
            None
        };
        Self {
            state,
            task,
            enabled,
        }
    }

    pub fn inc(&self) {
        self.state.current.fetch_add(1, Ordering::Relaxed);
    }

    /// Stop the bar, clear the live line, and return the elapsed time so
    /// the caller can fold it into their own "done" message instead of
    /// leaving a stale progress bar on screen.
    pub fn finish(mut self) -> Duration {
        let elapsed = self.state.started.elapsed();
        self.stop();
        elapsed
    }

    fn stop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
        if self.enabled {
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r\x1b[2K");
            let _ = err.flush();
        }
    }
}

impl Drop for ProgressBar {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Format a duration the same way the live spinners/bars render it
/// (`M:SS`), so callers can fold elapsed times into their own done
/// messages without each picking a different shape.
pub fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn render_bar(cur: u64, total: u64, width: usize) -> String {
    if total == 0 {
        return " ".repeat(width);
    }
    let frac = (cur as f64 / total as f64).clamp(0.0, 1.0);
    let filled = (frac * width as f64).round() as usize;
    let filled = filled.min(width);
    let mut s = String::with_capacity(width * 3);
    for _ in 0..filled {
        s.push('█');
    }
    for _ in filled..width {
        s.push('░');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_empty_when_zero() {
        let s = render_bar(0, 100, 10);
        assert_eq!(s.chars().count(), 10);
        assert!(s.chars().all(|c| c == '░'));
    }

    #[test]
    fn bar_full_when_complete() {
        let s = render_bar(100, 100, 10);
        assert_eq!(s.chars().count(), 10);
        assert!(s.chars().all(|c| c == '█'));
    }

    #[test]
    fn bar_handles_zero_total() {
        let s = render_bar(0, 0, 8);
        assert_eq!(s.chars().count(), 8);
    }
}
