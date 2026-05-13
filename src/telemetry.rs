//! Crash and error reporting.
//!
//! The public API (`init`, `report_error`, `is_enabled`) is uniform
//! whether the `telemetry` cargo feature is on or off — when it's off,
//! every function is a no-op so `main.rs` doesn't have to `#[cfg]`-gate
//! call sites.
//!
//! Design rationale and scope live in
//! `docs/01-crash-and-error-reporting.md`. The short version:
//!
//! * Sentry envelope protocol via the official `sentry` crate; backend
//!   is Sentry SaaS for now (DSN baked in at build time via the
//!   `WK_SENTRY_DSN` env var). No DSN → no-op even with the feature on.
//! * Scrubbing happens in `before_send` below. It is the load-bearing
//!   privacy layer; assume the SDK will try to capture more than we
//!   want and treat scrubbing as the policy point of record.
//! * Off-switches: env var `WK_TELEMETRY=0` and persisted
//!   `auth.json:telemetry=false`. Either disables for the run.

// Several helpers are referenced only from `cfg(feature = "telemetry")`
// blocks; without the feature they're effectively dead, which is
// expected.
#![cfg_attr(not(feature = "telemetry"), allow(dead_code))]

use std::time::Duration;

use crate::config;

/// `release` value for Sentry events. Composed at compile time so a
/// stripped binary still self-identifies. See `build.rs` for `WK_GIT_SHA`.
pub const CLI_RELEASE: &str = concat!(
    env!("CARGO_PKG_NAME"),
    "@",
    env!("CARGO_PKG_VERSION"),
    "+",
    env!("WK_GIT_SHA"),
);

/// DSN baked in at build time. Optional so source builds and forks
/// without the env var still compile (and run with telemetry inert).
#[cfg(feature = "telemetry")]
const SENTRY_DSN: Option<&str> = option_env!("WK_SENTRY_DSN");

/// Returned by [`init`]; on `Drop` flushes pending events with a short
/// timeout so the process doesn't hang on exit if the network is dead.
pub struct Guard {
    #[cfg(feature = "telemetry")]
    inner: Option<sentry::ClientInitGuard>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        #[cfg(feature = "telemetry")]
        {
            if let Some(g) = self.inner.take() {
                // 2s ceiling — slow enough to catch a real send on a
                // healthy network, short enough that a broken one
                // doesn't make `wk` feel hung at exit.
                let _ = g.flush(Some(Duration::from_secs(2)));
            }
        }
    }
}

/// Initialize the SDK if the feature is compiled in, a DSN is baked
/// in, and the user hasn't opted out. Otherwise returns an inert guard.
pub fn init() -> Guard {
    #[cfg(feature = "telemetry")]
    {
        if !is_enabled() {
            return Guard { inner: None };
        }
        let Some(dsn) = SENTRY_DSN else {
            return Guard { inner: None };
        };
        let Ok(parsed) = dsn.parse::<sentry::types::Dsn>() else {
            return Guard { inner: None };
        };

        let options = sentry::ClientOptions {
            dsn: Some(parsed),
            release: Some(CLI_RELEASE.into()),
            environment: Some(env_name().into()),
            // We never want the SDK guessing at PII (usernames, IPs,
            // env vars). `before_send` enforces the same thing, but
            // belt and suspenders.
            send_default_pii: false,
            attach_stacktrace: true,
            max_breadcrumbs: 50,
            sample_rate: 1.0,
            traces_sample_rate: 0.0,
            before_send: Some(std::sync::Arc::new(|event| Some(scrub_event(event)))),
            ..Default::default()
        };

        let guard = sentry::init(options);

        sentry::configure_scope(|scope| {
            scope.set_tag("client", "wavekat-cli");
            scope.set_tag("os.name", std::env::consts::OS);
            scope.set_tag("os.arch", std::env::consts::ARCH);
            if let Some(id) = config::ensure_install_id() {
                // `install_id` goes in `user.id` because Sentry uses
                // that for per-install dedup / counting, but it's not
                // a user identity — it's a random UUID generated on
                // first run. The scrubber forces username/email/ip to
                // empty so nothing else leaks here.
                scope.set_user(Some(sentry::User {
                    id: Some(id),
                    ..Default::default()
                }));
            }
        });

        Guard { inner: Some(guard) }
    }

    #[cfg(not(feature = "telemetry"))]
    {
        Guard {}
    }
}

/// Should we send anything? Consulted by `init` and again by
/// `report_error`; the env var check makes the env-var opt-out
/// effective even on already-initialized processes (unit tests, etc.).
pub fn is_enabled() -> bool {
    if !cfg!(feature = "telemetry") {
        return false;
    }
    if let Ok(v) = std::env::var("WK_TELEMETRY") {
        match v.trim().to_ascii_lowercase().as_str() {
            "" | "0" | "false" | "off" | "no" => return false,
            _ => {}
        }
    }
    let cfg = config::load_or_default();
    cfg.telemetry.unwrap_or(true)
}

/// Capture an `anyhow::Error` as a Sentry event, tagged with the
/// subcommand name and elapsed duration. No-op when disabled.
#[allow(unused_variables)]
pub fn report_error(command: &str, duration: Duration, err: &anyhow::Error) {
    #[cfg(feature = "telemetry")]
    {
        if !is_enabled() {
            return;
        }
        sentry::configure_scope(|scope| {
            scope.set_tag("cli.command", command);
            scope.set_tag("error.kind", classify_error(err));
            scope.set_extra(
                "duration_ms",
                serde_json::Value::from(duration.as_millis() as u64),
            );
        });
        sentry::integrations::anyhow::capture_anyhow(err);
    }
}

/// First-run notice — printed once on stderr, then suppressed via a
/// persisted flag. Called from `main` before dispatch; cheap.
pub fn maybe_print_first_run_notice() {
    if !cfg!(feature = "telemetry") {
        return;
    }
    let mut cfg = config::load_or_default();
    if cfg.telemetry_notice_shown {
        return;
    }
    if cfg.telemetry == Some(false) {
        // User already opted out somehow — don't pester.
        cfg.telemetry_notice_shown = true;
        let _ = config::save(&cfg);
        return;
    }
    eprintln!(
        "wk: anonymous error reports help us fix bugs faster.\n     \
         set WK_TELEMETRY=0 (or run `wk config telemetry off`) to opt out.\n     \
         what we collect: https://wavekat.com/docs/crashes"
    );
    cfg.telemetry_notice_shown = true;
    let _ = config::save(&cfg);
}

fn env_name() -> &'static str {
    if cfg!(debug_assertions) {
        "dev"
    } else {
        "production"
    }
}

/// Best-effort error categorization for the `error.kind` tag. Substring
/// matching is intentionally a placeholder — the `client::ClientError`
/// refactor (next PR) will replace this with structured info propagated
/// from the failure site.
fn classify_error(err: &anyhow::Error) -> &'static str {
    let s = format!("{err:?}").to_lowercase();
    if s.contains("not signed in") || s.contains("no credentials") {
        "auth_missing"
    } else if s.contains("decoding response") {
        "decode"
    } else if s.contains("dns")
        || s.contains("tls")
        || s.contains("connect")
        || s.contains("timed out")
    {
        "network"
    } else if s.contains(" 4") && s.contains(" /api/") {
        "http_4xx"
    } else if s.contains(" 5") && s.contains(" /api/") {
        "http_5xx"
    } else {
        "local_other"
    }
}

// ---------- scrubber ----------------------------------------------------

#[cfg(feature = "telemetry")]
fn scrub_event(mut event: sentry::protocol::Event<'static>) -> sentry::protocol::Event<'static> {
    use sentry::protocol::Value;
    use std::collections::BTreeMap;

    // Drop anything that could carry user identity beyond our own
    // anonymous install_id (which we set on the user object directly
    // in `init`).
    if let Some(user) = event.user.as_mut() {
        user.username = None;
        user.email = None;
        user.ip_address = None;
        user.other = BTreeMap::new();
    }
    event.server_name = None;
    // SDK fills `dist` from the host's hostname in some integrations.
    event.dist = None;

    // Request — wipe bodies and headers, templatize URL.
    if let Some(req) = event.request.as_mut() {
        req.data = None;
        req.cookies = None;
        req.headers = BTreeMap::new();
        req.env = BTreeMap::new();
        if let Some(url) = req.url.as_mut() {
            let templated_path = templatize_path(url.path());
            url.set_path(&templated_path);
            // Wipe query (we don't currently put IDs there, but the
            // SDK might attach arbitrary state).
            url.set_query(None);
            url.set_fragment(None);
        }
        req.query_string = None;
    }

    // Message + exception values: cap and redact home paths.
    if let Some(msg) = event.message.as_mut() {
        *msg = scrub_text(msg);
    }
    for ex in event.exception.values.iter_mut() {
        if let Some(v) = ex.value.as_mut() {
            *v = scrub_text(v);
        }
    }

    // Drop any extras that smell like creds — defense in depth; we
    // shouldn't be setting these but third-party integrations might.
    event.extra.retain(|k, _| !looks_sensitive(k));

    // Same for tag values (keys are ours, values are scrubbed for
    // accidental URL/path content).
    for (_, v) in event.tags.iter_mut() {
        *v = scrub_text(v);
    }

    // Strip request/url-shaped data out of breadcrumbs (the reqwest
    // integration likes to emit these).
    for crumb in event.breadcrumbs.iter_mut() {
        if let Some(msg) = crumb.message.as_mut() {
            *msg = scrub_text(msg);
        }
        let mut clean: BTreeMap<String, Value> = BTreeMap::new();
        for (k, v) in std::mem::take(&mut crumb.data).into_iter() {
            if looks_sensitive(&k) {
                continue;
            }
            let scrubbed = match v {
                Value::String(s) => Value::String(scrub_text(&s)),
                other => other,
            };
            clean.insert(k, scrubbed);
        }
        crumb.data = clean;
    }

    event
}

/// Replace ID-shaped segments in a URL path with `:id`. Takes the
/// path only (the request scrubber strips query and fragment
/// separately) so this never has to think about URL parsing.
#[cfg(feature = "telemetry")]
fn templatize_path(path: &str) -> String {
    path.split('/')
        .map(|seg| if looks_like_id(seg) { ":id" } else { seg })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(feature = "telemetry")]
fn looks_like_id(seg: &str) -> bool {
    // `proj_xyz`, `exp_xyz`, `model_xyz`, ... — a short lowercase
    // prefix, an underscore, then ≥3 chars of alphanumeric body.
    if let Some((prefix, body)) = seg.split_once('_') {
        if !prefix.is_empty()
            && prefix.len() <= 6
            && prefix.chars().all(|c| c.is_ascii_lowercase())
            && body.len() >= 3
            && body.chars().all(|c| c.is_ascii_alphanumeric())
        {
            return true;
        }
    }
    // UUID 8-4-4-4-12
    if seg.len() == 36 {
        let dashes: usize = seg.bytes().filter(|b| *b == b'-').count();
        if dashes == 4 && seg.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            return true;
        }
    }
    // Long hex strings (truncated content hashes, sha256 prefixes, …).
    if seg.len() >= 16 && seg.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    false
}

#[cfg(feature = "telemetry")]
fn looks_sensitive(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.contains("token")
        || k.contains("cookie")
        || k.contains("authorization")
        || k.contains("bearer")
        || k.contains("password")
        || k.contains("secret")
        || k.contains("api_key")
        || k.contains("apikey")
}

#[cfg(feature = "telemetry")]
fn scrub_text(s: &str) -> String {
    let redacted = redact_home(s);
    truncate(&redacted, 200)
}

#[cfg(feature = "telemetry")]
fn redact_home(s: &str) -> String {
    // Replace any literal occurrence of the user's home directory
    // prefix with `~`. Cheap, predictable, and avoids regexes.
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if !home_str.is_empty() && s.contains(home_str.as_ref()) {
            return s.replace(home_str.as_ref(), "~");
        }
    }
    s.to_string()
}

#[cfg(feature = "telemetry")]
fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n).collect();
    out.push('…');
    out
}

// ---------- tests -------------------------------------------------------

#[cfg(all(test, feature = "telemetry"))]
mod tests {
    use super::*;

    #[test]
    fn templatizes_proj_ids() {
        assert_eq!(
            templatize_path("/api/projects/proj_abc123/annotations"),
            "/api/projects/:id/annotations"
        );
    }

    #[test]
    fn templatizes_uuid() {
        assert_eq!(
            templatize_path("/api/exports/12345678-1234-1234-1234-1234567890ab"),
            "/api/exports/:id"
        );
    }

    #[test]
    fn templatizes_long_hex() {
        assert_eq!(
            templatize_path("/api/files/deadbeefcafebabe"),
            "/api/files/:id"
        );
    }

    #[test]
    fn keeps_non_id_segments() {
        assert_eq!(
            templatize_path("/api/auth/cli/tokens"),
            "/api/auth/cli/tokens"
        );
        // "v1" looks suspicious-ish but doesn't match any id rule.
        assert_eq!(templatize_path("/api/v1/me"), "/api/v1/me");
    }

    #[test]
    fn templatizes_multiple_segments() {
        assert_eq!(
            templatize_path("/api/projects/proj_abc/exports/exp_xyz"),
            "/api/projects/:id/exports/:id"
        );
    }

    #[test]
    fn looks_sensitive_catches_obvious() {
        assert!(looks_sensitive("Authorization"));
        assert!(looks_sensitive("cookie"));
        assert!(looks_sensitive("WK_TOKEN"));
        assert!(looks_sensitive("api_key"));
        assert!(!looks_sensitive("user_id"));
        assert!(!looks_sensitive("status"));
    }

    #[test]
    fn truncates_long_strings_with_ellipsis() {
        let s = "x".repeat(300);
        let out = truncate(&s, 200);
        // 200 chars + 1 ellipsis char.
        assert_eq!(out.chars().count(), 201);
    }

    #[test]
    fn truncate_passthrough_short() {
        assert_eq!(truncate("hello", 200), "hello");
    }

    #[test]
    fn redact_home_replaces_prefix() {
        // Only meaningful when dirs::home_dir() returns Some; on CI
        // boxes this is generally true. Falls through cleanly otherwise.
        if let Some(home) = dirs::home_dir() {
            let home_str = home.to_string_lossy().to_string();
            let example = format!("failed to write {home_str}/cache/x.parquet");
            let scrubbed = redact_home(&example);
            assert!(scrubbed.contains("~/cache/x.parquet"));
            assert!(!scrubbed.contains(&home_str));
        }
    }

    #[test]
    fn classify_recognizes_auth_missing() {
        let err = anyhow::anyhow!("not signed in — run `wk login` first");
        assert_eq!(classify_error(&err), "auth_missing");
    }

    #[test]
    fn classify_recognizes_decode() {
        let err = anyhow::anyhow!("decoding response from /api/me: {{bad}}");
        assert_eq!(classify_error(&err), "decode");
    }

    #[test]
    fn classify_falls_back_to_local_other() {
        let err = anyhow::anyhow!("something idiosyncratic");
        assert_eq!(classify_error(&err), "local_other");
    }
}
