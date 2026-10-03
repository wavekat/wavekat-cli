//! `wk admin …` — named, discoverable wrappers over the platform's
//! root-only read endpoints, for customer and product analysis.
//!
//! Every read is a GET that prints the endpoint's JSON unchanged. The
//! fleet-tag commands (`tags create`, `installs tag` / `untag`, and the
//! `users` pair) are the only writes; they live in `admin_tags.rs`.
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
use crate::commands::admin_tags::{self, SubjectAction, SubjectKind, TagCreate, TagSubject};
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
    /// Fleet tags for reviewing installs and users (`/api/admin/voice/fleet-tags`)
    Tags {
        #[command(subcommand)]
        command: TagsCmd,
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
    /// One user's story — days of events and tag changes, newest first
    /// (`GET /api/admin/voice/stories/users/{userId}`)
    Story(WithId),
    /// Tags on one user, removed ones included
    /// (`GET /api/admin/voice/fleet-tag-assignments`)
    Tags(SubjectOnly),
    /// Put a tag on a user; a no-op if it's already there
    /// (`POST /api/admin/voice/fleet-tag-assignments`)
    Tag(TagSubject),
    /// Take a tag off a user; a no-op if it isn't there
    /// (`DELETE /api/admin/voice/fleet-tag-assignments/{id}`)
    Untag(TagSubject),
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
    /// One install's story — days of events and tag changes, newest first
    /// (`GET /api/admin/voice/stories/installs/{installId}`)
    Story(WithId),
    /// Tags on one install, removed ones included
    /// (`GET /api/admin/voice/fleet-tag-assignments`)
    Tags(SubjectOnly),
    /// Put a tag on an install; a no-op if it's already there
    /// (`POST /api/admin/voice/fleet-tag-assignments`)
    Tag(TagSubject),
    /// Take a tag off an install; a no-op if it isn't there
    /// (`DELETE /api/admin/voice/fleet-tag-assignments/{id}`)
    Untag(TagSubject),
}

#[derive(Subcommand)]
pub enum TagsCmd {
    /// The tag vocabulary with live counts (`GET /api/admin/voice/fleet-tags`;
    /// `-q includeArchived=true` for archived ones)
    List(QueryOnly),
    /// Create a tag (`POST /api/admin/voice/fleet-tags`)
    Create(TagCreate),
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

/// Just an id: the install-subject commands accept either install id,
/// so this deliberately has no `--install-id` switch.
#[derive(ClapArgs)]
pub struct SubjectOnly {
    /// Install id (row id or client install id) or user id
    #[arg(value_parser = resource_id)]
    id: String,
    /// Accepted for consistency with the other read commands; output is
    /// always JSON.
    #[arg(long, hide = true)]
    json: bool,
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
    let result = match route(cmd) {
        Plan::Get(path, query) => get_and_print(&client, &path, &query).await,
        Plan::CreateTag(args) => admin_tags::create_tag(&client, args).await,
        Plan::Subject(kind, id, action) => {
            admin_tags::run_subject(&client, kind, &id, action).await
        }
    };
    result.map_err(|e| {
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

/// What a parsed command does. Plain reads map straight to an endpoint;
/// the tag commands need lookups first (tag name → id, install row id →
/// client install id), so they're handed to `admin_tags`.
enum Plan {
    Get(String, QueryArgs),
    CreateTag(TagCreate),
    Subject(SubjectKind, String, SubjectAction),
}

/// Map a parsed command to what it does. Kept pure so the command →
/// path table is unit-tested without a server.
fn route(cmd: Cmd) -> Plan {
    use Plan::Get;
    match cmd {
        Cmd::Users { command } => match command {
            UsersCmd::List(a) => Get("/api/users".into(), a.query),
            UsersCmd::Show(a) => Get(format!("/api/users/{}", seg(&a.id)), a.query),
            UsersCmd::Story(a) => user(a.id, SubjectAction::Story(a.query)),
            UsersCmd::Tags(a) => user(a.id, SubjectAction::Tags),
            UsersCmd::Tag(a) => user(a.id, SubjectAction::Tag(a.tag)),
            UsersCmd::Untag(a) => user(a.id, SubjectAction::Untag(a.tag)),
        },
        Cmd::Tags { command } => match command {
            TagsCmd::List(a) => Get(format!("{VOICE}/fleet-tags"), a.query),
            TagsCmd::Create(a) => Plan::CreateTag(a),
        },
        Cmd::Installs { command } => match command {
            InstallsCmd::Metrics(a) => Get(format!("{VOICE}/installs/metrics"), a.query),
            InstallsCmd::List(a) => Get(format!("{VOICE}/installs"), a.query),
            InstallsCmd::Show(a) if a.install_id => Get(
                format!("{VOICE}/installs/by-install-id/{}", seg(&a.id)),
                a.query,
            ),
            InstallsCmd::Show(a) => Get(format!("{VOICE}/installs/{}", seg(&a.id)), a.query),
            InstallsCmd::Events(a) => Get(
                format!("{VOICE}/installs/by-install-id/{}/events", seg(&a.id)),
                a.query,
            ),
            InstallsCmd::Story(a) => install(a.id, SubjectAction::Story(a.query)),
            InstallsCmd::Tags(a) => install(a.id, SubjectAction::Tags),
            InstallsCmd::Tag(a) => install(a.id, SubjectAction::Tag(a.tag)),
            InstallsCmd::Untag(a) => install(a.id, SubjectAction::Untag(a.tag)),
        },
        Cmd::Usage(a) => Get(format!("{VOICE}/usage"), a.query),
        Cmd::Downloads { command } => match command {
            DownloadsCmd::Metrics(a) => Get(format!("{VOICE}/downloads/metrics"), a.query),
            DownloadsCmd::List(a) => Get(format!("{VOICE}/downloads"), a.query),
        },
        Cmd::Prompts { command } => match command {
            PromptsCmd::Usage(a) => Get(format!("{VOICE}/prompt-usage"), a.query),
            PromptsCmd::Callers(a) => Get(format!("{VOICE}/prompt-callers"), a.query),
            PromptsCmd::Events(a) => Get(format!("{VOICE}/prompt-events"), a.query),
        },
        Cmd::Geo(a) => Get("/api/admin/geo".into(), a.query),
        Cmd::CliUsage(a) => Get("/api/admin/cli-usage".into(), a.query),
        Cmd::Spec(a) => Get("/api/openapi.json".into(), a.query),
    }
}

fn install(id: String, action: SubjectAction) -> Plan {
    Plan::Subject(SubjectKind::Install, id, action)
}

fn user(id: String, action: SubjectAction) -> Plan {
    Plan::Subject(SubjectKind::User, id, action)
}

/// Reject ids that `seg` can't make safe: URL parsers resolve `.` and
/// `..` (even percent-encoded) as dot segments, which would walk the
/// request up to a parent endpoint.
pub(crate) fn resource_id(raw: &str) -> Result<String, String> {
    match raw {
        "" => Err("id must not be empty".into()),
        "." | ".." => Err(format!("`{raw}` is not a valid id")),
        _ => Ok(raw.to_string()),
    }
}

/// Percent-encode an id for use as one path segment, so an id holding
/// `/`, `?` or `#` can't reach a different endpoint.
pub(crate) fn seg(id: &str) -> String {
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
        match route(h.cmd) {
            Plan::Get(path, _) => path,
            _ => panic!("argv {argv:?} should be a plain read"),
        }
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
    fn tag_commands_route_to_the_subject_they_name() {
        let plan = |argv: &[&str]| {
            let h =
                Harness::try_parse_from(std::iter::once("wk-admin").chain(argv.iter().copied()))
                    .expect("argv should parse");
            route(h.cmd)
        };
        assert_eq!(path_of(&["tags", "list"]), "/api/admin/voice/fleet-tags");
        assert!(matches!(
            plan(&["tags", "create", "reviewed", "--color", "green"]),
            Plan::CreateTag(a) if a.name == "reviewed"
                && a.color == crate::commands::admin_tags::TagColor::Green
        ));
        assert!(matches!(
            plan(&["installs", "tag", "abc", "reviewed"]),
            Plan::Subject(SubjectKind::Install, id, SubjectAction::Tag(t))
                if id == "abc" && t == "reviewed"
        ));
        assert!(matches!(
            plan(&["installs", "untag", "abc", "reviewed"]),
            Plan::Subject(SubjectKind::Install, _, SubjectAction::Untag(_))
        ));
        assert!(matches!(
            plan(&["installs", "tags", "abc"]),
            Plan::Subject(SubjectKind::Install, _, SubjectAction::Tags)
        ));
        assert!(matches!(
            plan(&["installs", "story", "abc", "-q", "limit=5"]),
            Plan::Subject(SubjectKind::Install, _, SubjectAction::Story(_))
        ));
        assert!(matches!(
            plan(&["users", "tag", "u1", "reviewed"]),
            Plan::Subject(SubjectKind::User, id, SubjectAction::Tag(_)) if id == "u1"
        ));
        assert!(matches!(
            plan(&["users", "story", "u1"]),
            Plan::Subject(SubjectKind::User, _, SubjectAction::Story(_))
        ));
    }

    #[test]
    fn tag_create_rejects_an_unknown_colour() {
        assert!(Harness::try_parse_from([
            "wk-admin", "tags", "create", "reviewed", "--color", "orange"
        ])
        .is_err());
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
        let Plan::Get(_, q) = route(h.cmd) else {
            panic!("geo should be a plain read")
        };
        assert_eq!(
            crate::commands::api::parse_query(&q.query).unwrap(),
            vec![("days".to_string(), "30".to_string())]
        );
    }
}
