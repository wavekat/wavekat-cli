//! `wk exports …` — manage dataset exports defined in `wavekat-platform`'s
//! `docs/06-export.md`.
//!
//! Layout mirrors `projects.rs` and `annotations.rs`: one Cmd enum dispatched
//! by `run()`, plus helpers. The smart-turn Parquet adapter lives next door
//! in `exports_smart_turn.rs` so this file stays focused on the HTTP surface.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::{Args as ClapArgs, Subcommand};
use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::client::Client;
use crate::commands::exports_smart_turn::{self, AdaptOptions};
use crate::progress::{with_spinner, ProgressBar};
use crate::style;

#[derive(Subcommand)]
pub enum Cmd {
    /// List exports for a project (`GET /api/projects/{id}/exports`)
    List(ListArgs),
    /// Show one export (`GET /api/exports/{id}`)
    Show(ShowArgs),
    /// Create a new export (`POST /api/projects/{id}/exports`)
    Create(CreateArgs),
    /// Soft-delete an export (`DELETE /api/exports/{id}`)
    Delete(DeleteArgs),
    /// Download an export's manifest + clips into a local directory
    Download(DownloadArgs),
    /// Convert a downloaded export into another format
    Adapt {
        #[command(subcommand)]
        command: AdaptCmd,
    },
}

#[derive(Subcommand)]
pub enum AdaptCmd {
    /// Emit a HuggingFace `datasets`-loadable Parquet shard set for
    /// Pipecat smart-turn (binary endpoint task).
    SmartTurn(AdaptSmartTurnArgs),
}

#[derive(ClapArgs)]
pub struct ListArgs {
    /// Project id (uuid)
    project_id: String,
    #[arg(long, default_value_t = 1)]
    page: u32,
    #[arg(long, default_value_t = 20)]
    page_size: u32,
    /// Print raw JSON instead of a table
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs)]
pub struct ShowArgs {
    /// Export id (uuid)
    export_id: String,
    /// Print raw JSON instead of a summary
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs)]
pub struct DeleteArgs {
    /// Export id (uuid)
    export_id: String,
    /// Skip the y/N confirmation
    #[arg(long, short = 'y')]
    yes: bool,
}

#[derive(ClapArgs)]
pub struct CreateArgs {
    /// Project id (uuid)
    project_id: String,
    /// Human-readable export name (shown in lists)
    #[arg(long)]
    name: String,
    /// Optional description
    #[arg(long)]
    description: Option<String>,
    /// Label set id to filter on (defaults to the project's active set)
    #[arg(long)]
    label_set_id: Option<String>,
    /// Review status to include. Repeat to allow several. One of:
    /// `approved`, `rejected`, `needs_fix`, `unreviewed`. Default:
    /// `approved` only.
    #[arg(long = "review-status")]
    review_statuses: Vec<String>,
    /// Restrict to specific label keys (e.g. `end_of_turn`). Repeatable.
    #[arg(long = "label-key")]
    label_keys: Vec<String>,
    /// Restrict to annotations created by these labeller user ids. Repeatable.
    #[arg(long = "labeller-id")]
    labeller_ids: Vec<i64>,
    /// Lower bound on annotation `createdAt` (RFC 3339).
    #[arg(long)]
    created_at_from: Option<String>,
    /// Upper bound on annotation `createdAt` (RFC 3339).
    #[arg(long)]
    created_at_to: Option<String>,
    /// Split policy. One of `random`, `by_source_file`, `by_labeller`.
    #[arg(long, default_value = "random")]
    split: String,
    /// Seed for the deterministic shuffle / partition.
    #[arg(long, default_value_t = 42)]
    seed: i64,
    /// Train/validation/test ratios as a comma-separated triple summing to 1.
    /// When `--use-reserved-test-files` is set, pass a *pair* (e.g.
    /// `0.9,0.1`) — the third slot is implicitly 0 because the test split
    /// is filled from reserved files only.
    #[arg(long, default_value = "0.8,0.1,0.1")]
    ratios: String,
    /// Use the project's reserved files as the entire test split. The
    /// non-reserved rows are split between train and validation only —
    /// see docs/08-test-set-reservation.md. Omit to disable explicitly.
    #[arg(
        long = "use-reserved-test-files",
        overrides_with = "no_use_reserved_test_files"
    )]
    use_reserved_test_files: bool,
    /// Force-disable the reservation flow even if the project has
    /// reserved files. Primarily for scripted exports that want the
    /// classic 3-way ratio behaviour regardless of project state.
    #[arg(
        long = "no-use-reserved-test-files",
        overrides_with = "use_reserved_test_files"
    )]
    no_use_reserved_test_files: bool,
    /// Print the new export row as raw JSON.
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs)]
pub struct DownloadArgs {
    /// Export id (uuid)
    export_id: String,
    /// Output directory (created if missing). Defaults to `./<export-id>`.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Re-download every clip even if a file with the same name already
    /// exists. Default behaviour skips clips already present.
    #[arg(long)]
    force: bool,
    /// Maximum number of clips to fetch in parallel. The platform side
    /// streams each clip through a Cloudflare Worker, which scales
    /// across isolates — so concurrency is a real win, but cranking it
    /// past ~16 starts hitting Worker subrequest budgets without
    /// meaningfully improving wall time. Default 8.
    #[arg(long, default_value_t = 8)]
    concurrency: usize,
}

#[derive(ClapArgs)]
pub struct AdaptSmartTurnArgs {
    /// Path to a downloaded export directory (contains `manifest.jsonl`
    /// and `clips/`). Use `--manifest` + `--clips-dir` if your layout
    /// differs.
    #[arg(long)]
    export_dir: Option<PathBuf>,
    /// Path to a `manifest.jsonl` (use with `--clips-dir`).
    #[arg(long)]
    manifest: Option<PathBuf>,
    /// Directory containing the clip wavs.
    #[arg(long)]
    clips_dir: Option<PathBuf>,
    /// Output directory (created if missing).
    #[arg(long)]
    out: PathBuf,
    /// ISO-639-1 language tag, e.g. `zh`. Stored on every example.
    #[arg(long)]
    language: String,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ExportRow {
    id: String,
    project_id: String,
    name: String,
    description: Option<String>,
    status: String,
    #[serde(default)]
    filter: serde_json::Value,
    #[serde(default)]
    split_policy: serde_json::Value,
    #[serde(default)]
    label_set_snapshot: serde_json::Value,
    r2_prefix: String,
    manifest_sha256: Option<String>,
    clip_count: Option<i64>,
    total_bytes: Option<i64>,
    created_by: i64,
    created_by_login: Option<String>,
    created_at: String,
    ready_at: Option<String>,
    error_message: Option<String>,
    #[serde(default)]
    can_download: bool,
    #[serde(default)]
    can_delete: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListResponse {
    exports: Vec<ExportRow>,
    page: u32,
    page_size: u32,
    total: u32,
    total_pages: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    page: u32,
    page_size: u32,
}

// Wire shape for POST /api/projects/{id}/exports — kept structurally
// identical to the platform's `CreateExportBody` zod schema.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    filter: CreateFilter<'a>,
    split_policy: CreateSplitPolicy<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateFilter<'a> {
    label_set_id: &'a str,
    review_statuses: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label_keys: Option<Vec<&'a str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    labeller_ids: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at_from: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at_to: Option<&'a str>,
    /// Mirrors the platform's `useReservedTestFiles` filter flag — when
    /// set, the server pulls the test split from the project's reserved
    /// files and snapshots their ids into the export `filter` for
    /// reproducibility (docs/08-test-set-reservation.md §4).
    #[serde(skip_serializing_if = "Option::is_none")]
    use_reserved_test_files: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateSplitPolicy<'a> {
    kind: &'a str,
    seed: i64,
    ratios: [f64; 3],
}

// Subset of the project detail response we need for `--label-set-id`
// defaulting in `wk exports create`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectDetail {
    active_label_set_id: Option<String>,
}

pub async fn run(cmd: Cmd) -> Result<()> {
    let client = Client::from_config()?;
    match cmd {
        Cmd::List(args) => list(&client, args).await,
        Cmd::Show(args) => show(&client, args).await,
        Cmd::Create(args) => create(&client, args).await,
        Cmd::Delete(args) => delete(&client, args).await,
        Cmd::Download(args) => download(&client, args).await,
        Cmd::Adapt { command } => match command {
            AdaptCmd::SmartTurn(args) => adapt_smart_turn(args).await,
        },
    }
}

async fn list(client: &Client, args: ListArgs) -> Result<()> {
    let path = format!("/api/projects/{}/exports", args.project_id);
    let query = ListQuery {
        page: args.page,
        page_size: args.page_size,
    };
    if args.json {
        let v: serde_json::Value = client.get_json_query(&path, &query).await?;
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let resp: ListResponse = client.get_json_query(&path, &query).await?;
    if resp.exports.is_empty() {
        println!("No exports.");
        return Ok(());
    }
    // Print the full UUID — exports IDs are arguments to the next command in
    // the pipeline (`wk exports download <id>` / `… show <id>`), so the user
    // needs to copy-paste them. Truncating to 8 chars (as we do for
    // annotations, where the id is just a row marker) defeats that.
    println!(
        "{}  {}  {}  {}  {}",
        style::bold(&format!("{:<38}", "ID")),
        style::bold(&format!("{:<32}", "NAME")),
        style::bold(&format!("{:<10}", "STATUS")),
        style::bold(&format!("{:<8}", "CLIPS")),
        style::bold("CREATED"),
    );
    for e in &resp.exports {
        let name = truncate(&e.name, 32);
        let clips = e
            .clip_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".to_string());
        println!(
            "{}  {name:<32}  {}  {}  {}",
            style::dim(&format!("{:<38}", e.id)),
            style::bold(&format!("{:<10}", colour_status_text(&e.status))),
            style::dim(&format!("{clips:<8}")),
            style::dim(&e.created_at),
        );
        if let Some(err) = e.error_message.as_deref() {
            println!(
                "{}  {}",
                style::dim(&format!("{:<38}", "")),
                style::red(&format!("error: {err}")),
            );
        }
    }
    println!(
        "\n{}",
        style::dim(&format!(
            "Page {}/{} · {} export(s) total · pageSize {}",
            resp.page, resp.total_pages, resp.total, resp.page_size
        )),
    );
    if resp.page < resp.total_pages {
        println!(
            "{} wk exports list {} --page {}{}",
            style::dim("Next:"),
            args.project_id,
            resp.page + 1,
            if resp.page_size != 20 {
                format!(" --page-size {}", resp.page_size)
            } else {
                String::new()
            },
        );
    }
    Ok(())
}

async fn show(client: &Client, args: ShowArgs) -> Result<()> {
    let path = format!("/api/exports/{}", args.export_id);
    let v: serde_json::Value = client.get_json(&path).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("-").to_string();
    let label = |s: &str| style::dim(&format!("{s:<14}"));
    println!("{} {}", label("id:"), style::dim(&s("id")));
    println!("{} {}", label("name:"), style::bold(&s("name")));
    if let Some(desc) = v.get("description").and_then(|x| x.as_str()) {
        if !desc.trim().is_empty() {
            println!("{} {desc}", label("description:"));
        }
    }
    println!("{} {}", label("project:"), s("projectId"));
    println!(
        "{} {}",
        label("status:"),
        colour_status(v.get("status").and_then(|x| x.as_str()).unwrap_or("-")),
    );
    if let Some(n) = v.get("clipCount").and_then(|x| x.as_i64()) {
        println!("{} {n}", label("clips:"));
    }
    if let Some(b) = v.get("totalBytes").and_then(|x| x.as_i64()) {
        println!("{} {}", label("bytes:"), human_bytes(b));
    }
    if let Some(sh) = v.get("manifestSha256").and_then(|x| x.as_str()) {
        println!("{} {}", label("manifest:"), style::dim(sh));
    }
    println!("{} {}", label("created:"), s("createdAt"));
    if let Some(r) = v.get("readyAt").and_then(|x| x.as_str()) {
        println!("{} {r}", label("ready:"));
    }
    if let Some(err) = v.get("errorMessage").and_then(|x| x.as_str()) {
        println!("{} {}", label("error:"), style::red(err));
    }
    if let Some(by) = v.get("createdByLogin").and_then(|x| x.as_str()) {
        println!("{} {by}", label("created by:"));
    }
    if let Some(filter) = v.get("filter") {
        println!(
            "{} {}",
            label("filter:"),
            style::dim(&serde_json::to_string(filter).unwrap_or_default()),
        );
    }
    if let Some(sp) = v.get("splitPolicy") {
        println!(
            "{} {}",
            label("split:"),
            style::dim(&serde_json::to_string(sp).unwrap_or_default()),
        );
    }
    Ok(())
}

async fn create(client: &Client, args: CreateArgs) -> Result<()> {
    // Resolve label set: explicit flag wins; otherwise look up the project's
    // active set so the common case is one fewer flag to remember.
    let label_set_id = match args.label_set_id.clone() {
        Some(id) => id,
        None => {
            let p: ProjectDetail = client
                .get_json(&format!("/api/projects/{}", args.project_id))
                .await?;
            p.active_label_set_id.ok_or_else(|| {
                anyhow!("project has no active label set; pass --label-set-id explicitly")
            })?
        }
    };

    let review_statuses_owned: Vec<String> = if args.review_statuses.is_empty() {
        vec!["approved".to_string()]
    } else {
        args.review_statuses.clone()
    };
    let review_statuses: Vec<&str> = review_statuses_owned.iter().map(|s| s.as_str()).collect();

    let label_keys: Option<Vec<&str>> = if args.label_keys.is_empty() {
        None
    } else {
        Some(args.label_keys.iter().map(|s| s.as_str()).collect())
    };
    let labeller_ids = if args.labeller_ids.is_empty() {
        None
    } else {
        Some(args.labeller_ids.clone())
    };

    // Reservation flag tri-state: explicit `--use-reserved-test-files`
    // wins, explicit `--no-…` wins, otherwise we leave it `None` and
    // the platform applies its default (which today is "off" — auto-on
    // is a UI-only convenience driven by the summary endpoint).
    let use_reserved = if args.use_reserved_test_files {
        Some(true)
    } else if args.no_use_reserved_test_files {
        Some(false)
    } else {
        None
    };
    let reservation_on = use_reserved == Some(true);
    let ratios = parse_ratios(&args.ratios, reservation_on)?;

    let body = CreateBody {
        name: &args.name,
        description: args.description.as_deref(),
        filter: CreateFilter {
            label_set_id: &label_set_id,
            review_statuses,
            label_keys,
            labeller_ids,
            created_at_from: args.created_at_from.as_deref(),
            created_at_to: args.created_at_to.as_deref(),
            use_reserved_test_files: use_reserved,
        },
        split_policy: CreateSplitPolicy {
            kind: &args.split,
            seed: args.seed,
            ratios,
        },
    };
    let path = format!("/api/projects/{}/exports", args.project_id);
    // The platform builds the export synchronously (copies every clip
    // into R2 before responding), so this can take seconds-to-minutes.
    // Spinner is purely a liveness signal — there's no progress field
    // to render until the platform exposes one.
    let (resp, elapsed) = with_spinner(
        "Creating export…",
        client.post_json::<serde_json::Value, _>(&path, &body),
    )
    .await;
    let resp = resp?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }
    let id = resp.get("id").and_then(|x| x.as_str()).unwrap_or("");
    let status = resp.get("status").and_then(|x| x.as_str()).unwrap_or("");
    let clips = resp
        .get("clipCount")
        .and_then(|x| x.as_i64())
        .map(|n| n.to_string())
        .unwrap_or_else(|| "—".to_string());
    println!(
        "{} {} {}",
        style::bold("Created"),
        style::dim(id),
        style::dim(&format!("in {}", crate::progress::format_elapsed(elapsed))),
    );
    println!("  status: {}  clips: {}", colour_status(status), clips);
    println!("  next:   {}", style::dim(&format!("wk exports show {id}")),);
    Ok(())
}

async fn delete(client: &Client, args: DeleteArgs) -> Result<()> {
    if !args.yes {
        eprintln!(
            "About to soft-delete export {}. The clips remain in R2 until the cleanup sweep purges them (~7 days).",
            args.export_id,
        );
        eprintln!("Re-run with --yes to confirm.");
        return Err(anyhow!("aborted"));
    }
    client
        .delete(&format!("/api/exports/{}", args.export_id))
        .await?;
    println!("{} {}", style::bold("Deleted"), style::dim(&args.export_id));
    Ok(())
}

async fn download(client: &Client, args: DownloadArgs) -> Result<()> {
    let out_dir = args
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from(&args.export_id));
    let clips_dir = out_dir.join("clips");
    fs::create_dir_all(&clips_dir)
        .await
        .with_context(|| format!("creating {}", clips_dir.display()))?;

    // Fetch the export row first so we 404 early on missing/forbidden ids
    // before opening the manifest writer. Surfaces auth failures up
    // front and confirms the export is in `ready` state before we touch
    // disk.
    let row: ExportRow = client
        .get_json(&format!("/api/exports/{}", args.export_id))
        .await?;
    if row.status != "ready" {
        return Err(anyhow!(
            "export status is `{}` — only `ready` exports can be downloaded",
            row.status
        ));
    }

    let manifest_path = out_dir.join("manifest.jsonl");
    let mut manifest_file = fs::File::create(&manifest_path)
        .await
        .with_context(|| format!("creating {}", manifest_path.display()))?;
    let bytes = client
        .get_stream_to(
            &format!("/api/exports/{}/manifest", args.export_id),
            &mut manifest_file,
        )
        .await?;
    eprintln!(
        "{} manifest.jsonl ({})",
        style::dim("downloaded"),
        human_bytes(bytes as i64),
    );

    // Read the manifest into a Vec up front so we have a known total
    // for the progress bar and a flat work-list to drive
    // `buffer_unordered`. Manifests are line-oriented JSON, so this is
    // cheap even for tens of thousands of clips.
    let manifest = fs::File::open(&manifest_path).await?;
    let mut lines = BufReader::new(manifest).lines();
    let mut ids: Vec<String> = Vec::new();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line)
            .with_context(|| format!("parsing manifest line: {line}"))?;
        let ann_id = v
            .get("annotationId")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow!("manifest line missing annotationId: {line}"))?;
        ids.push(ann_id.to_string());
    }
    let total = ids.len() as u64;

    // Partition into already-on-disk vs. needs-fetch. Skipped clips
    // tick the bar immediately so `--force=false` resumes look fast
    // and the displayed progress matches the on-disk reality.
    let mut skipped: u64 = 0;
    let mut pending: Vec<String> = Vec::with_capacity(ids.len());
    let bar = ProgressBar::new("downloading clips", total);
    for ann_id in ids {
        let dest = clips_dir.join(format!("{ann_id}.wav"));
        if dest.exists() && !args.force {
            skipped += 1;
            bar.inc();
            continue;
        }
        pending.push(ann_id);
    }

    let concurrency = args.concurrency.max(1);
    let export_id = args.export_id.as_str();
    let mut downloads = stream::iter(pending.into_iter().map(|ann_id| {
        let bar = &bar;
        let clips_dir = &clips_dir;
        async move {
            let dest = clips_dir.join(format!("{ann_id}.wav"));
            let mut f = fs::File::create(&dest)
                .await
                .with_context(|| format!("creating {}", dest.display()))?;
            client
                .get_stream_to(&format!("/api/exports/{export_id}/clips/{ann_id}"), &mut f)
                .await?;
            bar.inc();
            Ok::<(), anyhow::Error>(())
        }
    }))
    .buffer_unordered(concurrency);

    while let Some(r) = downloads.next().await {
        // Bail on first error. In-flight downloads in the buffer are
        // dropped (and therefore aborted) when we leave this scope —
        // partial files they left behind get retried on the next run.
        r?;
    }
    drop(downloads);
    let elapsed = bar.finish();

    let downloaded = total - skipped;
    eprintln!(
        "{} {downloaded} clip(s){} {}",
        style::dim("downloaded"),
        if skipped > 0 {
            format!(" ({skipped} already on disk, skipped)")
        } else {
            String::new()
        },
        style::dim(&format!("in {}", crate::progress::format_elapsed(elapsed))),
    );
    println!("{}", out_dir.display());
    Ok(())
}

async fn adapt_smart_turn(args: AdaptSmartTurnArgs) -> Result<()> {
    let (manifest, clips_dir) = resolve_adapt_inputs(&args)?;
    let written = exports_smart_turn::run(AdaptOptions {
        manifest_path: manifest,
        clips_dir,
        out_dir: args.out.clone(),
        language: args.language.clone(),
    })
    .await?;
    let parts: Vec<String> = written
        .split_counts
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    println!(
        "wrote {} examples to {} ({}) {}",
        written.total,
        args.out.display(),
        parts.join(", "),
        style::dim(&format!(
            "in {}",
            crate::progress::format_elapsed(written.elapsed)
        )),
    );
    Ok(())
}

fn resolve_adapt_inputs(args: &AdaptSmartTurnArgs) -> Result<(PathBuf, PathBuf)> {
    if let Some(dir) = args.export_dir.as_ref() {
        let manifest = dir.join("manifest.jsonl");
        let clips = dir.join("clips");
        if !manifest.exists() {
            return Err(anyhow!("{}: manifest.jsonl not found", manifest.display()));
        }
        if !clips.exists() {
            return Err(anyhow!("{}: clips/ not found", clips.display()));
        }
        return Ok((manifest, clips));
    }
    let manifest = args
        .manifest
        .clone()
        .ok_or_else(|| anyhow!("--manifest is required when --export-dir is not given"))?;
    let clips = args
        .clips_dir
        .clone()
        .ok_or_else(|| anyhow!("--clips-dir is required when --export-dir is not given"))?;
    if !manifest.exists() {
        return Err(anyhow!("{}: manifest not found", manifest.display()));
    }
    if !clips.exists() {
        return Err(anyhow!("{}: clips directory not found", clips.display()));
    }
    Ok((manifest, clips))
}

/// Parse `--ratios`. When `reservation_on` is true, accepts either a
/// 2-tuple (`train,val`) — implicit test=0 — or a 3-tuple whose third
/// slot is exactly 0. Anything else is rejected so the caller cannot
/// silently waste a non-zero `test` ratio on the reservation flow.
/// When `reservation_on` is false, requires the classic 3-tuple summing
/// to 1.
fn parse_ratios(raw: &str, reservation_on: bool) -> Result<[f64; 3]> {
    let parts: Vec<&str> = raw.split(',').map(|s| s.trim()).collect();
    if !(parts.len() == 2 || parts.len() == 3) {
        return Err(anyhow!(
            "--ratios expects two or three comma-separated numbers (got {raw:?})"
        ));
    }
    let nums: Vec<f64> = parts
        .iter()
        .map(|s| {
            s.parse::<f64>()
                .with_context(|| format!("parsing ratio {s:?}"))
        })
        .collect::<Result<Vec<_>>>()?;

    if reservation_on {
        let (train, val) = match nums.as_slice() {
            [t, v] => (*t, *v),
            [t, v, test] => {
                if test.abs() > 1e-6 {
                    return Err(anyhow!(
                        "--ratios test slot must be 0 when --use-reserved-test-files is set (got {test})"
                    ));
                }
                (*t, *v)
            }
            _ => unreachable!(),
        };
        let sum = train + val;
        if (sum - 1.0).abs() > 1e-6 {
            return Err(anyhow!(
                "--ratios train+val must sum to 1.0 with --use-reserved-test-files (got {sum})"
            ));
        }
        return Ok([train, val, 0.0]);
    }

    if nums.len() != 3 {
        return Err(anyhow!(
            "--ratios expects three comma-separated numbers without --use-reserved-test-files (got {raw:?})"
        ));
    }
    let sum: f64 = nums.iter().sum();
    if (sum - 1.0).abs() > 1e-6 {
        return Err(anyhow!("--ratios must sum to 1.0 (got {sum})"));
    }
    Ok([nums[0], nums[1], nums[2]])
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    } else {
        s.to_string()
    }
}

fn colour_status(s: &str) -> String {
    match s {
        "ready" => style::green(s),
        "running" | "pending" => style::yellow(s),
        "failed" => style::red(s),
        _ => s.to_string(),
    }
}

fn colour_status_text(s: &str) -> String {
    // Used inside fixed-width `{:<N}` cells where ANSI codes would distort
    // padding — return the plain text and let the caller apply styling
    // around the already-padded cell.
    s.to_string()
}

fn human_bytes(n: i64) -> String {
    let n = n as f64;
    if n < 1024.0 {
        return format!("{n:.0} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut v = n / 1024.0;
    let mut idx = 0;
    while v >= 1024.0 && idx + 1 < units.len() {
        v /= 1024.0;
        idx += 1;
    }
    format!("{v:.1} {}", units[idx])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_accept_default() {
        let r = parse_ratios("0.8,0.1,0.1", false).unwrap();
        assert_eq!(r, [0.8, 0.1, 0.1]);
    }

    #[test]
    fn ratios_reject_off_total() {
        assert!(parse_ratios("0.5,0.5,0.5", false).is_err());
    }

    #[test]
    fn ratios_reject_two_tuple_without_reservation() {
        assert!(parse_ratios("0.5,0.5", false).is_err());
    }

    #[test]
    fn ratios_accept_two_tuple_with_reservation() {
        let r = parse_ratios("0.9,0.1", true).unwrap();
        assert_eq!(r, [0.9, 0.1, 0.0]);
    }

    #[test]
    fn ratios_accept_three_tuple_with_zero_test_when_reserved() {
        let r = parse_ratios("0.8,0.2,0", true).unwrap();
        assert_eq!(r, [0.8, 0.2, 0.0]);
    }

    #[test]
    fn ratios_reject_nonzero_test_when_reserved() {
        // The third slot is meaningless when reservation is on — reject so
        // a typo can't silently throw away a chunk of training data.
        assert!(parse_ratios("0.7,0.1,0.2", true).is_err());
    }

    #[test]
    fn ratios_reject_train_val_off_one_when_reserved() {
        assert!(parse_ratios("0.5,0.4", true).is_err());
    }

    #[test]
    fn human_bytes_scales() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(2048), "2.0 KB");
        assert_eq!(human_bytes(5_500_000), "5.2 MB");
    }

    #[test]
    fn truncate_clips_long() {
        let out = truncate("abcdefghij", 5);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 5);
    }
}
