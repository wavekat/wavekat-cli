//! `wk admin …` — named, discoverable wrappers over the platform's
//! root-only read endpoints, for customer and product analysis.
//!
//! Every command is a GET that prints the endpoint's JSON unchanged.
//! Filters pass through as `-q key=value` rather than as bespoke flags:
//! the server's query schemas move faster than CLI releases, and the
//! authoritative list is the OpenAPI document (`wk admin spec`). The
//! CLI's job here is naming and discovery, not re-describing each API.
//!
//! The platform gates all of these on the global `root` role; a
//! non-root token gets a 403 from the server, not a client-side check.

use anyhow::Result;
use clap::{Args as ClapArgs, Subcommand};

use crate::client::Client;
use crate::commands::api::{get_and_print, QueryArgs};

#[derive(Subcommand)]
pub enum Cmd {
    /// Platform users (`/api/users`)
    Users {
        #[command(subcommand)]
        command: UsersCmd,
    },
    /// WaveKat Voice installs (`/api/admin/voice/installs`)
    Installs {
        #[command(subcommand)]
        command: InstallsCmd,
    },
    /// Voice usage-event funnel and breakdown (`GET /api/admin/voice/usage`)
    Usage(QueryOnly),
    /// WaveKat Voice downloads (`/api/admin/voice/downloads`)
    Downloads {
        #[command(subcommand)]
        command: DownloadsCmd,
    },
    /// Voice-prompt generation usage (`/api/admin/voice/prompt-*`)
    Prompts {
        #[command(subcommand)]
        command: PromptsCmd,
    },
    /// Customer geography from sign-ins (`GET /api/admin/geo`)
    Geo(QueryOnly),
    /// `wk` CLI usage summary (`GET /api/admin/cli-usage`)
    CliUsage(QueryOnly),
    /// Print the platform's OpenAPI document (`GET /api/openapi.json`) —
    /// the reference for every endpoint and its `-q` parameters
    Spec(QueryOnly),
}

#[derive(Subcommand)]
pub enum UsersCmd {
    /// List users, paginated (`GET /api/users`; e.g. `-q q=alice -q tier=pro`)
    List(QueryOnly),
    /// One user's admin activity summary (`GET /api/users/{id}`)
    Show(WithId),
}

#[derive(Subcommand)]
pub enum InstallsCmd {
    /// Aggregated install metrics (`GET /api/admin/voice/installs/metrics`)
    Metrics(QueryOnly),
    /// List installs, paginated (`GET /api/admin/voice/installs`)
    List(QueryOnly),
    /// One install with its linked users (`GET /api/admin/voice/installs/{id}`)
    Show(InstallShow),
    /// One install's usage events, cursor-paginated
    /// (`GET /api/admin/voice/installs/by-install-id/{installId}/events`)
    Events(WithId),
}

#[derive(Subcommand)]
pub enum DownloadsCmd {
    /// Aggregated download metrics (`GET /api/admin/voice/downloads/metrics`)
    Metrics(QueryOnly),
    /// List individual downloads (`GET /api/admin/voice/downloads`)
    List(QueryOnly),
}

#[derive(Subcommand)]
pub enum PromptsCmd {
    /// Headline prompt-generation counters (`GET /api/admin/voice/prompt-usage`)
    Usage(QueryOnly),
    /// Per-caller usage rollup (`GET /api/admin/voice/prompt-callers`)
    Callers(QueryOnly),
    /// Raw request log, cursor-paginated (`GET /api/admin/voice/prompt-events`)
    Events(QueryOnly),
}

#[derive(ClapArgs)]
pub struct QueryOnly {
    #[command(flatten)]
    query: QueryArgs,
}

#[derive(ClapArgs)]
pub struct WithId {
    /// Resource id
    #[arg(value_parser = resource_id)]
    id: String,
    #[command(flatten)]
    query: QueryArgs,
}

#[derive(ClapArgs)]
pub struct InstallShow {
    /// Install row id, or the client-reported install id with `--install-id`
    #[arg(value_parser = resource_id)]
    id: String,
    /// Treat `<ID>` as the client-reported install id
    /// (`GET /api/admin/voice/installs/by-install-id/{installId}`)
    #[arg(long)]
    install_id: bool,
    #[command(flatten)]
    query: QueryArgs,
}

pub async fn run(cmd: Cmd) -> Result<()> {
    let client = Client::from_config()?;
    let (path, query) = route(cmd);
    get_and_print(&client, &path, &query).await.map_err(|e| {
        if is_forbidden(&e) {
            e.context(
                "forbidden — `wk admin` needs a token from an account with the global root role",
            )
        } else {
            e
        }
    })
}

fn is_forbidden(e: &anyhow::Error) -> bool {
    matches!(
        e.downcast_ref::<wavekat_platform_client::Error>(),
        Some(wavekat_platform_client::Error::Http { status: 403, .. })
    )
}

const VOICE: &str = "/api/admin/voice";

/// Map a parsed command to the endpoint it reads. Kept pure so the
/// command → path table is unit-tested without a server.
fn route(cmd: Cmd) -> (String, QueryArgs) {
    match cmd {
        Cmd::Users { command } => match command {
            UsersCmd::List(a) => ("/api/users".into(), a.query),
            UsersCmd::Show(a) => (format!("/api/users/{}", seg(&a.id)), a.query),
        },
        Cmd::Installs { command } => match command {
            InstallsCmd::Metrics(a) => (format!("{VOICE}/installs/metrics"), a.query),
            InstallsCmd::List(a) => (format!("{VOICE}/installs"), a.query),
            InstallsCmd::Show(a) if a.install_id => (
                format!("{VOICE}/installs/by-install-id/{}", seg(&a.id)),
                a.query,
            ),
            InstallsCmd::Show(a) => (format!("{VOICE}/installs/{}", seg(&a.id)), a.query),
            InstallsCmd::Events(a) => (
                format!("{VOICE}/installs/by-install-id/{}/events", seg(&a.id)),
                a.query,
            ),
        },
        Cmd::Usage(a) => (format!("{VOICE}/usage"), a.query),
        Cmd::Downloads { command } => match command {
            DownloadsCmd::Metrics(a) => (format!("{VOICE}/downloads/metrics"), a.query),
            DownloadsCmd::List(a) => (format!("{VOICE}/downloads"), a.query),
        },
        Cmd::Prompts { command } => match command {
            PromptsCmd::Usage(a) => (format!("{VOICE}/prompt-usage"), a.query),
            PromptsCmd::Callers(a) => (format!("{VOICE}/prompt-callers"), a.query),
            PromptsCmd::Events(a) => (format!("{VOICE}/prompt-events"), a.query),
        },
        Cmd::Geo(a) => ("/api/admin/geo".into(), a.query),
        Cmd::CliUsage(a) => ("/api/admin/cli-usage".into(), a.query),
        Cmd::Spec(a) => ("/api/openapi.json".into(), a.query),
    }
}

/// Reject ids that `seg` can't make safe: URL parsers resolve `.` and
/// `..` (even percent-encoded) as dot segments, which would walk the
/// request up to a parent endpoint.
fn resource_id(raw: &str) -> Result<String, String> {
    match raw {
        "" => Err("id must not be empty".into()),
        "." | ".." => Err(format!("`{raw}` is not a valid id")),
        _ => Ok(raw.to_string()),
    }
}

/// Percent-encode an id for use as one path segment, so an id holding
/// `/`, `?` or `#` can't reach a different endpoint.
fn seg(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for b in id.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Harness {
        #[command(subcommand)]
        cmd: Cmd,
    }

    fn path_of(argv: &[&str]) -> String {
        let h = Harness::try_parse_from(std::iter::once("wk-admin").chain(argv.iter().copied()))
            .expect("argv should parse");
        route(h.cmd).0
    }

    #[test]
    fn routes_every_command_to_its_endpoint() {
        let cases: &[(&[&str], &str)] = &[
            (&["users", "list"], "/api/users"),
            (&["users", "show", "42"], "/api/users/42"),
            (
                &["installs", "metrics"],
                "/api/admin/voice/installs/metrics",
            ),
            (&["installs", "list"], "/api/admin/voice/installs"),
            (
                &["installs", "show", "abc"],
                "/api/admin/voice/installs/abc",
            ),
            (
                &["installs", "show", "abc", "--install-id"],
                "/api/admin/voice/installs/by-install-id/abc",
            ),
            (
                &["installs", "events", "abc"],
                "/api/admin/voice/installs/by-install-id/abc/events",
            ),
            (&["usage"], "/api/admin/voice/usage"),
            (
                &["downloads", "metrics"],
                "/api/admin/voice/downloads/metrics",
            ),
            (&["downloads", "list"], "/api/admin/voice/downloads"),
            (&["prompts", "usage"], "/api/admin/voice/prompt-usage"),
            (&["prompts", "callers"], "/api/admin/voice/prompt-callers"),
            (&["prompts", "events"], "/api/admin/voice/prompt-events"),
            (&["geo"], "/api/admin/geo"),
            (&["cli-usage"], "/api/admin/cli-usage"),
            (&["spec"], "/api/openapi.json"),
        ];
        for (argv, want) in cases {
            assert_eq!(path_of(argv), *want, "argv {argv:?}");
        }
    }

    #[test]
    fn ids_cannot_escape_their_path_segment() {
        assert_eq!(
            path_of(&["users", "show", "../admin/settings"]),
            "/api/users/..%2Fadmin%2Fsettings"
        );
        assert_eq!(
            path_of(&["users", "show", "1?role=root"]),
            "/api/users/1%3Frole%3Droot"
        );
    }

    #[test]
    fn dot_segment_ids_are_rejected() {
        for bad in ["", ".", ".."] {
            assert!(
                Harness::try_parse_from(["wk-admin", "users", "show", bad]).is_err(),
                "id {bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn query_flags_pass_through() {
        let h = Harness::try_parse_from(["wk-admin", "geo", "-q", "days=30", "--json"]).unwrap();
        let (_, q) = route(h.cmd);
        assert_eq!(
            crate::commands::api::parse_query(&q.query).unwrap(),
            vec![("days".to_string(), "30".to_string())]
        );
    }
}
