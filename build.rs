use std::process::Command;

// Capture a short git SHA at build time and expose it as the
// `WK_GIT_SHA` env var so the binary can include it in its `release`
// identity for crash reporting (see docs/01-crash-and-error-reporting.md).
//
// We deliberately *don't* fail the build when git isn't available or
// the working tree isn't a checkout — published crate sources, tarball
// builds, and forks-without-history should all still compile. In those
// cases the SHA falls back to "unknown".
fn main() {
    let sha = git_short_sha().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=WK_GIT_SHA={sha}");

    // Re-run only when HEAD moves. `cargo:rerun-if-changed` keys off
    // file mtime; pointing at .git/HEAD and the packed-refs file covers
    // both detached and branch-tracked checkouts. Missing files are
    // ignored by cargo, so this is safe outside a git checkout too.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/packed-refs");
    println!("cargo:rerun-if-env-changed=WK_GIT_SHA_OVERRIDE");
}

fn git_short_sha() -> Option<String> {
    if let Ok(forced) = std::env::var("WK_GIT_SHA_OVERRIDE") {
        if !forced.trim().is_empty() {
            return Some(forced.trim().to_string());
        }
    }
    let out = Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
