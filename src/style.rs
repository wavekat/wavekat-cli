// Tiny ANSI styling helpers. Self-contained so we don't drag in a colour
// crate just to dim a few labels. Output is automatically plain when
// stdout isn't a TTY (so piping into `jq` / files / other tools is
// untouched) and when the `NO_COLOR` env var is set per
// <https://no-color.org>. `CLICOLOR_FORCE=1` forces colour on, useful in
// CI where stdout is captured but you still want the codes.

use std::io::IsTerminal;
use std::sync::OnceLock;

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if std::env::var("CLICOLOR_FORCE").is_ok_and(|v| v != "0") {
            return true;
        }
        std::io::stdout().is_terminal()
    })
}

fn wrap(code: &str, body: &str) -> String {
    if enabled() {
        format!("\x1b[{code}m{body}\x1b[0m")
    } else {
        body.to_string()
    }
}

pub fn bold(s: &str) -> String {
    wrap("1", s)
}
pub fn dim(s: &str) -> String {
    wrap("2", s)
}
pub fn red(s: &str) -> String {
    wrap("31", s)
}
pub fn green(s: &str) -> String {
    wrap("32", s)
}
pub fn yellow(s: &str) -> String {
    wrap("33", s)
}
pub fn cyan(s: &str) -> String {
    wrap("36", s)
}
pub fn magenta(s: &str) -> String {
    wrap("35", s)
}

/// Colour a review status string. Unknown / unreviewed render dim.
pub fn review(status: Option<&str>) -> String {
    match status {
        Some("approved") => green("approved"),
        Some("rejected") => red("rejected"),
        Some("needs_fix") => yellow("needs_fix"),
        Some(other) => other.to_string(),
        None => dim("—"),
    }
}

/// Colour a user role string.
pub fn role(s: &str) -> String {
    match s {
        "root" => magenta("root"),
        "user" => green("user"),
        "none" => yellow("none"),
        _ => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests assert on substrings rather than exact bytes, so they
    // pass whether `enabled()` decides colour is on or off — the cached
    // OnceLock means we can't reliably flip the mode mid-test-binary.

    #[test]
    fn bold_preserves_payload() {
        assert!(bold("hello").contains("hello"));
    }

    #[test]
    fn review_known_states_contain_label() {
        assert!(review(Some("approved")).contains("approved"));
        assert!(review(Some("rejected")).contains("rejected"));
        assert!(review(Some("needs_fix")).contains("needs_fix"));
    }

    #[test]
    fn review_unknown_passes_through() {
        // Unknown statuses must render verbatim — no swallowing.
        assert_eq!(review(Some("draft")), "draft");
    }

    #[test]
    fn review_none_renders_em_dash() {
        assert!(review(None).contains('—'));
    }

    #[test]
    fn role_unknown_passes_through() {
        assert_eq!(role("admin"), "admin");
    }
}
