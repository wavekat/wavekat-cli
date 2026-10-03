//! Fleet tags — the review side of `wk admin`. A root reviewer (or an
//! agent working for one) reads an install's story, then records what
//! they found as a tag: `reviewed`, `signin-failed`, … Counting tags
//! across the fleet is how findings turn into priorities.
//!
//! Unlike the rest of `wk admin`, some of these write. They stay narrow
//! on purpose — create a tag, put one on a subject, take one off — and
//! every write is idempotent, so an agent can retry without checking
//! first. There is deliberately no free-text note: a finding that has
//! no tag gets a new tag, not a note about a person.

use anyhow::{anyhow, bail, Result};
use clap::{Args as ClapArgs, ValueEnum};
use serde_json::{json, Value};

use crate::client::Client;
use crate::commands::admin::seg;
use crate::commands::api::{get_and_print, QueryArgs};

const VOICE: &str = "/api/admin/voice";

/// The colours the platform accepts (`VoiceFleetTagCreate.color`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum TagColor {
    Gray,
    Red,
    Amber,
    Green,
    Blue,
    Violet,
    Pink,
}

impl TagColor {
    fn as_str(self) -> &'static str {
        match self {
            TagColor::Gray => "gray",
            TagColor::Red => "red",
            TagColor::Amber => "amber",
            TagColor::Green => "green",
            TagColor::Blue => "blue",
            TagColor::Violet => "violet",
            TagColor::Pink => "pink",
        }
    }
}

#[derive(ClapArgs)]
pub struct TagCreate {
    /// Tag name: lowercase letters, digits and `-`, up to 32 characters
    /// (e.g. `signin-failed`). Can't be renamed later.
    pub name: String,
    /// Chip colour on the admin pages
    #[arg(long, value_enum, default_value = "gray")]
    pub color: TagColor,
    /// What the tag means, for whoever applies it next (≤ 200 chars)
    #[arg(long)]
    pub description: Option<String>,
}

#[derive(ClapArgs)]
pub struct TagSubject {
    /// The install or user to tag (for an install, either id works —
    /// the row id from the admin URL or the client install id)
    #[arg(value_parser = crate::commands::admin::resource_id)]
    pub id: String,
    /// Tag name, as `wk admin tags list` prints it
    pub tag: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubjectKind {
    Install,
    User,
}

impl SubjectKind {
    fn as_str(self) -> &'static str {
        match self {
            SubjectKind::Install => "install",
            SubjectKind::User => "user",
        }
    }
}

/// A command that needs the subject's id resolved before it can run.
pub enum SubjectAction {
    Story(QueryArgs),
    Tags,
    Tag(String),
    Untag(String),
}

pub async fn create_tag(client: &Client, args: TagCreate) -> Result<()> {
    let body = json!({
        "name": args.name,
        "color": args.color.as_str(),
        "description": args.description,
    });
    let tag: Value = match client
        .post_json(&format!("{VOICE}/fleet-tags"), &body)
        .await
    {
        Ok(created) => created,
        // A 409 is either "name taken" or "too many live tags". Taken
        // means the tag exists — print it, as `installs tag` does for an
        // existing assignment — so agents can create-then-apply blindly.
        Err(e) if status_of(&e) == Some(409) => {
            let tags = list_tags(client).await?;
            match find_by_name(&tags, &args.name) {
                Some(existing) => existing.clone(),
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    println!("{}", serde_json::to_string_pretty(&tag)?);
    Ok(())
}

pub async fn run_subject(
    client: &Client,
    kind: SubjectKind,
    id: &str,
    action: SubjectAction,
) -> Result<()> {
    let id = match kind {
        SubjectKind::Install => resolve_install_id(client, id).await?,
        // The platform 404s a tag on an unknown user, and the user id is
        // the only id a user has.
        SubjectKind::User => id.to_string(),
    };
    match action {
        SubjectAction::Story(query) => {
            let path = match kind {
                SubjectKind::Install => format!("{VOICE}/stories/installs/{}", seg(&id)),
                SubjectKind::User => format!("{VOICE}/stories/users/{}", seg(&id)),
            };
            get_and_print(client, &path, &query).await
        }
        SubjectAction::Tags => {
            let assignments = load_assignments(client, kind, &id).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({ "assignments": assignments }))?
            );
            Ok(())
        }
        SubjectAction::Tag(name) => {
            let tag_id = resolve_tag_id(client, &name).await?;
            let body = json!({ "subjectKind": kind.as_str(), "subjectId": id, "tagId": tag_id });
            let assignment: Value = client
                .post_json(&format!("{VOICE}/fleet-tag-assignments"), &body)
                .await?;
            println!("{}", serde_json::to_string_pretty(&assignment)?);
            Ok(())
        }
        SubjectAction::Untag(name) => {
            let assignments = load_assignments(client, kind, &id).await?;
            let removed = match live_assignment_id(&assignments, &name) {
                Some(assignment_id) => {
                    client
                        .delete(&format!(
                            "{VOICE}/fleet-tag-assignments/{}",
                            seg(&assignment_id)
                        ))
                        .await?;
                    true
                }
                None => false,
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({ "ok": true, "removed": removed }))?
            );
            Ok(())
        }
    }
}

/// Tags address an install by its client-reported install id, and the
/// platform accepts any string there without checking it — so a row id
/// pasted from an admin URL would silently tag nothing. Accept either id
/// and refuse one that names no install.
async fn resolve_install_id(client: &Client, id: &str) -> Result<String> {
    match client
        .get_json::<Value>(&format!("{VOICE}/installs/by-install-id/{}", seg(id)))
        .await
    {
        Ok(body) => return install_id_of(&body),
        Err(e) if !is_not_found(&e) => return Err(e),
        Err(_) => {}
    }
    match client
        .get_json::<Value>(&format!("{VOICE}/installs/{}", seg(id)))
        .await
    {
        Ok(body) => install_id_of(&body),
        Err(e) if is_not_found(&e) => bail!("no install with id `{id}`"),
        Err(e) => Err(e),
    }
}

fn install_id_of(body: &Value) -> Result<String> {
    body.pointer("/install/installId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("platform response has no `install.installId`"))
}

fn is_not_found(e: &anyhow::Error) -> bool {
    status_of(e) == Some(404)
}

fn status_of(e: &anyhow::Error) -> Option<u16> {
    match e.downcast_ref::<wavekat_platform_client::Error>() {
        Some(wavekat_platform_client::Error::Http { status, .. }) => Some(*status),
        _ => None,
    }
}

async fn load_assignments(client: &Client, kind: SubjectKind, id: &str) -> Result<Vec<Value>> {
    let body: Value = client
        .get_json_query(
            &format!("{VOICE}/fleet-tag-assignments"),
            &[("subjectKind", kind.as_str()), ("subjectId", id)],
        )
        .await?;
    Ok(body
        .get("assignments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

async fn resolve_tag_id(client: &Client, name: &str) -> Result<String> {
    pick_tag(&list_tags(client).await?, name)
}

/// Every tag, archived ones included.
async fn list_tags(client: &Client) -> Result<Vec<Value>> {
    let body: Value = client
        .get_json_query(
            &format!("{VOICE}/fleet-tags"),
            &[("includeArchived", "true")],
        )
        .await?;
    Ok(body
        .get("tags")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn find_by_name<'a>(tags: &'a [Value], name: &str) -> Option<&'a Value> {
    tags.iter()
        .find(|t| t.get("name").and_then(Value::as_str) == Some(name))
}

/// Find a live tag by name, explaining the two ways that goes wrong.
fn pick_tag(tags: &[Value], name: &str) -> Result<String> {
    match find_by_name(tags, name) {
        Some(t) if !t.get("archivedAt").is_none_or(Value::is_null) => {
            bail!("tag `{name}` is archived — unarchive it on the admin Tags page to apply it")
        }
        Some(t) => t
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("tag `{name}` has no id in the platform response")),
        None => {
            let live: Vec<&str> = tags
                .iter()
                .filter(|t| t.get("archivedAt").is_none_or(Value::is_null))
                .filter_map(|t| t.get("name").and_then(Value::as_str))
                .collect();
            let known = if live.is_empty() {
                "there are no tags yet".to_string()
            } else {
                format!("existing tags: {}", live.join(", "))
            };
            bail!("no tag named `{name}` ({known}) — create it with `wk admin tags create {name}`")
        }
    }
}

/// The id of the subject's live (not removed) assignment of tag `name`.
fn live_assignment_id(assignments: &[Value], name: &str) -> Option<String> {
    assignments
        .iter()
        .filter(|a| a.get("removedAt").is_none_or(Value::is_null))
        .find(|a| a.pointer("/tag/name").and_then(Value::as_str) == Some(name))
        .and_then(|a| a.get("id").and_then(Value::as_str))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags() -> Vec<Value> {
        vec![
            json!({ "id": "t1", "name": "reviewed", "archivedAt": null }),
            json!({ "id": "t2", "name": "old-idea", "archivedAt": "2026-09-01T00:00:00Z" }),
            json!({ "id": "t3", "name": "signin-failed", "archivedAt": null }),
        ]
    }

    #[test]
    fn pick_tag_finds_a_live_tag_by_name() {
        assert_eq!(pick_tag(&tags(), "signin-failed").unwrap(), "t3");
    }

    #[test]
    fn pick_tag_refuses_an_archived_tag() {
        let err = pick_tag(&tags(), "old-idea").unwrap_err().to_string();
        assert!(err.contains("archived"), "{err}");
    }

    #[test]
    fn pick_tag_lists_live_tags_when_the_name_is_unknown() {
        let err = pick_tag(&tags(), "nope").unwrap_err().to_string();
        assert!(err.contains("reviewed, signin-failed"), "{err}");
        assert!(
            !err.contains("old-idea"),
            "archived tags aren't offered: {err}"
        );
        assert!(err.contains("wk admin tags create nope"), "{err}");
    }

    #[test]
    fn pick_tag_with_no_tags_says_so() {
        let err = pick_tag(&[], "reviewed").unwrap_err().to_string();
        assert!(err.contains("no tags yet"), "{err}");
    }

    #[test]
    fn live_assignment_skips_removed_ones() {
        let assignments = vec![
            json!({ "id": "a1", "tag": { "name": "reviewed" }, "removedAt": "2026-10-01T00:00:00Z" }),
            json!({ "id": "a2", "tag": { "name": "reviewed" }, "removedAt": null }),
            json!({ "id": "a3", "tag": { "name": "signin-failed" }, "removedAt": null }),
        ];
        assert_eq!(
            live_assignment_id(&assignments, "reviewed").as_deref(),
            Some("a2")
        );
        assert_eq!(live_assignment_id(&assignments[..1], "reviewed"), None);
        assert_eq!(live_assignment_id(&assignments, "missing"), None);
    }

    #[test]
    fn install_id_is_read_from_the_show_response() {
        let body = json!({ "clients": [], "install": { "id": "row", "installId": "client" } });
        assert_eq!(install_id_of(&body).unwrap(), "client");
        assert!(install_id_of(&json!({})).is_err());
    }

    #[test]
    fn colour_names_match_the_platform_enum() {
        let names: Vec<&str> = TagColor::value_variants()
            .iter()
            .map(|c| c.as_str())
            .collect();
        assert_eq!(
            names,
            ["gray", "red", "amber", "green", "blue", "violet", "pink"]
        );
    }
}
