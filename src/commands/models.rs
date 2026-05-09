//! `wk models …` — push and inspect trained models registered against
//! a project, mirroring the platform's model registry (see
//! `wavekat-platform/docs/11-model-registry.md`).
//!
//! Layout mirrors `exports.rs`: one `Cmd` enum dispatched by `run()`,
//! plus per-subcommand helpers. Hot path is `push`, which:
//!
//!   1. Reads `results.json` and each `--artifact <path>`.
//!   2. Computes sha256 + byte length for every artifact.
//!   3. POSTs `/api/projects/{id}/models` with the declared metadata
//!      (name, recipe, lineage, sha256s, artifacts list).
//!   4. PUTs each artifact body — to the presigned R2 URL when the
//!      platform returned one, or to the worker-proxy fallback path
//!      otherwise (local dev / tests).
//!   5. POSTs `/api/models/{id}/finalize` so the platform HEADs the
//!      uploads, verifies sha256, projects metric scalars, and flips
//!      the row to `ready`.
//!
//! Idempotent on `(project, training_export, recipe, artifact_sha256)`:
//! a server 200 + `deduplicated: true` short-circuits without re-uploading.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use clap::{Args as ClapArgs, Subcommand};
use serde::{Deserialize, Serialize};
// `ModelRow` round-trips through both Deserialize (read response) and
// Serialize (re-emit on `--json`), so derive both.
use sha2::{Digest, Sha256};
use tokio::fs;

use crate::client::Client;
use crate::progress::{with_spinner, ProgressBar};
use crate::style;

#[derive(Subcommand)]
pub enum Cmd {
    /// Push a trained model to the platform (`POST /api/projects/{id}/models` + uploads + finalize)
    Push(PushArgs),
    /// List models for a project (`GET /api/projects/{id}/models`)
    List(ListArgs),
    /// Show one model (`GET /api/models/{id}`)
    Show(ShowArgs),
    /// Download model artifacts (`GET /api/models/{id}/download/{filename}`)
    Download(DownloadArgs),
    /// Soft-delete a model (`DELETE /api/models/{id}`)
    Delete(DeleteArgs),
}

#[derive(ClapArgs)]
pub struct PushArgs {
    /// Project id (uuid).
    #[arg(long)]
    project: String,
    /// Training export id (uuid).
    #[arg(long = "training-export")]
    training_export: String,
    /// Optional test export id (uuid). Models that scored against a
    /// frozen test set use this; without it, the platform treats
    /// `test.*` metrics as drawn from the training export's own test
    /// partition.
    #[arg(long = "test-export")]
    test_export: Option<String>,
    /// Optional parent model id — the model this run warm-started from
    /// (`--warm-start-from` on the lab side). Reserved for lineage
    /// trees; doesn't affect storage or metrics.
    #[arg(long = "parent-model")]
    parent_model: Option<String>,
    /// Recipe name (e.g. `specaugment`). Falls back to
    /// `results.json:recipe.name` when omitted.
    #[arg(long)]
    recipe: Option<String>,
    /// Path to the run's `results.json`. Required.
    #[arg(long)]
    results: PathBuf,
    /// Path to one model artifact (e.g. `model.onnx`,
    /// `model.int8.onnx`). Repeatable — every push needs at least one.
    #[arg(long = "artifact")]
    artifacts: Vec<PathBuf>,
    /// Human label (e.g. `0504-specaug`). Defaults to a generated
    /// name combining recipe + timestamp.
    #[arg(long)]
    name: Option<String>,
    /// Optional free-text description.
    #[arg(long)]
    description: Option<String>,
    /// Print the created model row as raw JSON.
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs)]
pub struct ListArgs {
    /// Project id (uuid)
    project_id: String,
    #[arg(long, default_value_t = 1)]
    page: u32,
    #[arg(long, default_value_t = 20)]
    page_size: u32,
    /// Restrict to models trained on this export.
    #[arg(long = "training-export")]
    training_export: Option<String>,
    /// Restrict to models that used this export as their test bench.
    #[arg(long = "test-export")]
    test_export: Option<String>,
    /// Restrict to a recipe name (e.g. `specaugment`).
    #[arg(long)]
    recipe: Option<String>,
    /// Restrict to one of `pending`, `uploading`, `ready`, `error`.
    #[arg(long)]
    status: Option<String>,
    /// Print raw JSON instead of a table.
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs)]
pub struct ShowArgs {
    /// Model id (uuid)
    model_id: String,
    /// Print raw JSON instead of a summary
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs)]
pub struct DownloadArgs {
    /// Model id (uuid)
    model_id: String,
    /// Output directory (created if missing). Defaults to `./<model-id>`.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Restrict to a single artifact filename (e.g. `model.int8.onnx`).
    /// Without this every artifact is downloaded.
    #[arg(long)]
    artifact: Option<String>,
    /// Re-download even if the file is already on disk.
    #[arg(long)]
    force: bool,
}

#[derive(ClapArgs)]
pub struct DeleteArgs {
    /// Model id (uuid)
    model_id: String,
    /// Skip the y/N confirmation
    #[arg(long, short = 'y')]
    yes: bool,
}

// ── Wire shapes ─────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    recipe_name: &'a str,
    training_export_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    test_export_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_model_id: Option<&'a str>,
    recipe_json: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics_json: Option<serde_json::Value>,
    artifact_sha256: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    int8_sha256: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git_sha: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git_dirty: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dataset_sha256: Option<&'a str>,
    artifacts: Vec<DeclaredArtifact<'a>>,
}

#[derive(Serialize)]
struct DeclaredArtifact<'a> {
    filename: &'a str,
    sha256: &'a str,
    bytes: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateResponse {
    model: ModelRow,
    uploads: Vec<UploadInstruction>,
    deduplicated: bool,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UploadInstruction {
    filename: String,
    /// Direct R2 PUT URL (with SigV4 in the query string). Null when
    /// the platform isn't configured with R2 access keys — the client
    /// falls back to `proxy_put_url`.
    put_url: Option<String>,
    proxy_put_url: String,
    #[allow(dead_code)]
    expires_at: Option<String>,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ModelRow {
    id: String,
    project_id: String,
    name: String,
    status: String,
    recipe_name: String,
    training_export_id: String,
    training_export_name: Option<String>,
    test_export_id: Option<String>,
    val_f1: Option<f64>,
    val_threshold: Option<f64>,
    test_f1: Option<f64>,
    test_f1_ci95_low: Option<f64>,
    test_f1_ci95_high: Option<f64>,
    test_ap: Option<f64>,
    artifact_sha256: Option<String>,
    int8_sha256: Option<String>,
    total_bytes: Option<i64>,
    created_by_login: Option<String>,
    created_at: String,
    error_message: Option<String>,
    artifacts: Vec<ArtifactRow>,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ArtifactRow {
    filename: String,
    sha256: String,
    byte_length: i64,
    #[allow(dead_code)]
    verified_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListResponse {
    models: Vec<ModelRow>,
    page: u32,
    page_size: u32,
    total: u32,
    total_pages: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery<'a> {
    page: u32,
    page_size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    training_export_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    test_export_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recipe_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'a str>,
}

pub async fn run(cmd: Cmd) -> Result<()> {
    let client = Client::from_config()?;
    match cmd {
        Cmd::Push(args) => push(&client, args).await,
        Cmd::List(args) => list(&client, args).await,
        Cmd::Show(args) => show(&client, args).await,
        Cmd::Download(args) => download(&client, args).await,
        Cmd::Delete(args) => delete(&client, args).await,
    }
}

// ── push ────────────────────────────────────────────────────────────────

async fn push(client: &Client, args: PushArgs) -> Result<()> {
    if args.artifacts.is_empty() {
        return Err(anyhow!(
            "at least one --artifact <path> is required (e.g. model.onnx)"
        ));
    }

    // results.json is the lab's run record (see
    // wavekat-lab/notebooks/smart-turn/docs/05-pipeline-wheel.md "Run
    // output contract"). We read it whole, send the parsed JSON as
    // `metricsJson`, and pull a few well-known sub-fields out for
    // first-class columns on the row.
    let results_text = fs::read_to_string(&args.results)
        .await
        .with_context(|| format!("reading {}", args.results.display()))?;
    let results: serde_json::Value = serde_json::from_str(&results_text)
        .with_context(|| format!("parsing {}: not valid JSON", args.results.display()))?;

    // Recipe name: explicit flag wins, otherwise fall back to
    // results.recipe.name. Erroring early here saves the user from a
    // 400 round-trip.
    let recipe_name_owned: String = match args.recipe.clone() {
        Some(s) => s,
        None => results
            .get("recipe")
            .and_then(|r| r.get("name"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                anyhow!(
                    "no --recipe given and {} has no `recipe.name` field",
                    args.results.display()
                )
            })?,
    };

    let recipe_json = results
        .get("recipe")
        .cloned()
        .unwrap_or(serde_json::Value::Object(Default::default()));

    // Provenance fields if the lab side recorded them (see doc 11
    // §"Run output contract"). Missing values stay null on the row.
    let git_sha = results
        .get("git")
        .and_then(|g| g.get("sha"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let git_dirty = results
        .get("git")
        .and_then(|g| g.get("dirty"))
        .and_then(|v| v.as_bool());
    let dataset_sha256 = results
        .get("dataset")
        .and_then(|d| d.get("sha256"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            results
                .get("export")
                .and_then(|e| e.get("manifestSha256"))
                .and_then(|v| v.as_str())
        })
        .map(|s| s.to_string());

    // Hash + size each artifact. We slurp the whole file into memory
    // because (a) the upload path also wants the bytes resident, and
    // (b) ONNX checkpoints we ship today are ≤50 MB. If a future
    // artifact gets gigabyte-scale, swap to a streaming hasher and a
    // streaming PUT.
    eprintln!(
        "{} {} artifact(s)…",
        style::dim("hashing"),
        args.artifacts.len()
    );
    let mut prepared: Vec<PreparedArtifact> = Vec::with_capacity(args.artifacts.len());
    for path in &args.artifacts {
        prepared.push(prepare_artifact(path).await?);
    }

    // Canonical FP32 sha picks the file named `model.onnx`. If no
    // file matches, we fall back to the first declared artifact and
    // warn — the doc says idempotency keys off the FP32 sha (§10 q4),
    // so naming matters.
    let fp32_sha = pick_canonical_sha(&prepared, "model.onnx");
    let int8_sha = pick_canonical_sha_optional(&prepared, "model.int8.onnx");

    let name_owned: String = args
        .name
        .clone()
        .unwrap_or_else(|| default_name(&recipe_name_owned));

    let body = CreateBody {
        name: &name_owned,
        description: args.description.as_deref(),
        recipe_name: &recipe_name_owned,
        training_export_id: &args.training_export,
        test_export_id: args.test_export.as_deref(),
        parent_model_id: args.parent_model.as_deref(),
        recipe_json,
        metrics_json: Some(results),
        artifact_sha256: &fp32_sha,
        int8_sha256: int8_sha.as_deref(),
        git_sha: git_sha.as_deref(),
        git_dirty,
        dataset_sha256: dataset_sha256.as_deref(),
        artifacts: prepared
            .iter()
            .map(|p| DeclaredArtifact {
                filename: &p.filename,
                sha256: &p.sha256,
                bytes: p.bytes.len() as u64,
            })
            .collect(),
    };

    let path = format!("/api/projects/{}/models", args.project);
    let (resp, _elapsed) = with_spinner(
        "Registering model…",
        client.post_json::<CreateResponse, _>(&path, &body),
    )
    .await;
    let resp = resp?;

    if resp.deduplicated {
        // Same triple + same canonical sha already pushed. The server
        // returns the existing row and an empty uploads list; print
        // the row and exit non-error so re-runs in CI don't fail.
        if args.json {
            println!("{}", serde_json::to_string_pretty(&resp.model)?);
        } else {
            println!(
                "{} {} {}",
                style::bold("Already pushed"),
                style::dim(&resp.model.id),
                style::dim("(idempotent re-push, nothing to upload)"),
            );
            print_summary(&resp.model);
        }
        return Ok(());
    }

    // Upload each artifact. We deliberately upload sequentially: each
    // artifact is small enough that pipelining is overkill, and the
    // single in-flight PUT keeps the progress bar honest.
    let total_bytes: u64 = prepared.iter().map(|p| p.bytes.len() as u64).sum();
    let bar = ProgressBar::new("uploading", total_bytes);
    for art in prepared {
        let upload = resp
            .uploads
            .iter()
            .find(|u| u.filename == art.filename)
            .ok_or_else(|| {
                anyhow!(
                    "server didn't return an upload URL for `{}` — push aborted",
                    art.filename
                )
            })?;
        if let Some(put_url) = upload.put_url.as_deref() {
            Client::put_presigned_bytes(put_url, art.bytes.clone()).await?;
        } else {
            client.put_proxy_bytes(&upload.proxy_put_url, art.bytes.clone()).await?;
        }
        bar.add(art.bytes.len() as u64);
    }
    bar.finish();

    let finalized: ModelRow = client
        .post_empty_returning_json(&format!("/api/models/{}/finalize", resp.model.id))
        .await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&finalized)?);
        return Ok(());
    }
    println!(
        "{} {}",
        style::bold("✅ model"),
        style::dim(&finalized.id),
    );
    print_summary(&finalized);
    println!(
        "  {}: {}",
        style::dim("url"),
        style::dim(&format!(
            "{}/projects/{}/models/{}",
            client.base_url_for_display(),
            finalized.project_id,
            finalized.id
        )),
    );
    Ok(())
}

struct PreparedArtifact {
    filename: String,
    sha256: String,
    bytes: Vec<u8>,
}

async fn prepare_artifact(path: &Path) -> Result<PreparedArtifact> {
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("artifact path has no filename: {}", path.display()))?
        .to_string();
    if !is_valid_filename(&filename) {
        return Err(anyhow!(
            "artifact filename `{filename}` contains characters the platform rejects (allowed: A-Za-z0-9._-, max 128)"
        ));
    }
    let bytes = fs::read(path)
        .await
        .with_context(|| format!("reading {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    Ok(PreparedArtifact {
        filename,
        sha256: hex,
        bytes,
    })
}

fn pick_canonical_sha(prepared: &[PreparedArtifact], wanted: &str) -> String {
    if let Some(p) = prepared.iter().find(|p| p.filename == wanted) {
        return p.sha256.clone();
    }
    // Fall back to the first artifact's sha. This keeps `wk models
    // push --artifact run.tar.gz` working when a recipe doesn't ship
    // a literal `model.onnx`, at the cost of a slightly weaker
    // idempotency guarantee for that case.
    prepared
        .first()
        .map(|p| p.sha256.clone())
        .unwrap_or_default()
}

fn pick_canonical_sha_optional(prepared: &[PreparedArtifact], wanted: &str) -> Option<String> {
    prepared
        .iter()
        .find(|p| p.filename == wanted)
        .map(|p| p.sha256.clone())
}

// Server-side validator: filename must match `[A-Za-z0-9._-]{1,128}`
// (mirrored from services/api/src/routes/models.ts FILENAME_RE). We
// re-check client-side so the error fires with the file path, not as
// a 400 from the API.
fn is_valid_filename(name: &str) -> bool {
    if name.is_empty() || name.len() > 128 {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

fn default_name(recipe: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{recipe}-{secs}")
}

// ── list ────────────────────────────────────────────────────────────────

async fn list(client: &Client, args: ListArgs) -> Result<()> {
    let path = format!("/api/projects/{}/models", args.project_id);
    let query = ListQuery {
        page: args.page,
        page_size: args.page_size,
        training_export_id: args.training_export.as_deref(),
        test_export_id: args.test_export.as_deref(),
        recipe_name: args.recipe.as_deref(),
        status: args.status.as_deref(),
    };
    if args.json {
        let v: serde_json::Value = client.get_json_query(&path, &query).await?;
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let resp: ListResponse = client.get_json_query(&path, &query).await?;
    if resp.models.is_empty() {
        println!("No models.");
        return Ok(());
    }
    println!(
        "{}  {}  {}  {}  {}  {}  {}",
        style::bold(&format!("{:<38}", "ID")),
        style::bold(&format!("{:<28}", "NAME")),
        style::bold(&format!("{:<14}", "RECIPE")),
        style::bold(&format!("{:<7}", "VAL F1")),
        style::bold(&format!("{:<7}", "TST F1")),
        style::bold(&format!("{:<10}", "STATUS")),
        style::bold("CREATED"),
    );
    for m in &resp.models {
        let name = truncate(&m.name, 28);
        let recipe = truncate(&m.recipe_name, 14);
        println!(
            "{}  {name:<28}  {recipe:<14}  {}  {}  {}  {}",
            style::dim(&format!("{:<38}", m.id)),
            format!("{:<7}", fmt_metric(m.val_f1)),
            format!("{:<7}", fmt_metric(m.test_f1)),
            style::bold(&format!("{:<10}", m.status)),
            style::dim(&m.created_at),
        );
    }
    println!(
        "\n{}",
        style::dim(&format!(
            "Page {}/{} · {} model(s) total · pageSize {}",
            resp.page, resp.total_pages, resp.total, resp.page_size
        )),
    );
    Ok(())
}

// ── show ────────────────────────────────────────────────────────────────

async fn show(client: &Client, args: ShowArgs) -> Result<()> {
    let path = format!("/api/models/{}", args.model_id);
    if args.json {
        let v: serde_json::Value = client.get_json(&path).await?;
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let row: ModelRow = client.get_json(&path).await?;
    print_summary(&row);
    Ok(())
}

fn print_summary(m: &ModelRow) {
    let label = |s: &str| style::dim(&format!("{s:<18}"));
    println!("  {} {}", label("name:"), style::bold(&m.name));
    println!("  {} {}", label("project:"), m.project_id);
    println!("  {} {}", label("status:"), m.status);
    println!("  {} {}", label("recipe:"), m.recipe_name);
    let train_label = m
        .training_export_name
        .as_deref()
        .unwrap_or(&m.training_export_id);
    println!("  {} {}", label("training export:"), train_label);
    if let Some(t) = m.test_export_id.as_deref() {
        println!("  {} {}", label("test export:"), t);
    }
    if let Some(f) = m.val_f1 {
        let thr = m
            .val_threshold
            .map(|t| format!(" @ thr {t:.2}"))
            .unwrap_or_default();
        println!("  {} {f:.3}{thr}", label("val F1:"));
    }
    if let Some(f) = m.test_f1 {
        let ci = match (m.test_f1_ci95_low, m.test_f1_ci95_high) {
            (Some(lo), Some(hi)) => format!(" ({lo:.3}–{hi:.3} CI95)"),
            _ => String::new(),
        };
        let ap = m.test_ap.map(|a| format!("  AP {a:.3}")).unwrap_or_default();
        println!("  {} {f:.3}{ci}{ap}", label("test F1:"));
    }
    if !m.artifacts.is_empty() {
        let parts: Vec<String> = m
            .artifacts
            .iter()
            .map(|a| format!("{} ({})", a.filename, human_bytes(a.byte_length)))
            .collect();
        println!("  {} {}", label("artifacts:"), parts.join("  "));
    }
    if let Some(s) = m.artifact_sha256.as_deref() {
        println!("  {} {}", label("fp32 sha256:"), style::dim(s));
    }
    if let Some(s) = m.int8_sha256.as_deref() {
        println!("  {} {}", label("int8 sha256:"), style::dim(s));
    }
    if let Some(b) = m.total_bytes {
        println!("  {} {}", label("total bytes:"), human_bytes(b));
    }
    if let Some(by) = m.created_by_login.as_deref() {
        println!("  {} @{by}", label("by:"));
    }
    println!("  {} {}", label("created:"), m.created_at);
    if let Some(err) = m.error_message.as_deref() {
        println!("  {} {}", label("error:"), style::red(err));
    }
}

// ── download ───────────────────────────────────────────────────────────

async fn download(client: &Client, args: DownloadArgs) -> Result<()> {
    let row: ModelRow = client
        .get_json(&format!("/api/models/{}", args.model_id))
        .await?;
    if row.status != "ready" {
        return Err(anyhow!(
            "model status is `{}` — only `ready` models can be downloaded",
            row.status
        ));
    }

    let out_dir = args
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from(&args.model_id));
    fs::create_dir_all(&out_dir)
        .await
        .with_context(|| format!("creating {}", out_dir.display()))?;

    let mut targets: Vec<&ArtifactRow> = Vec::new();
    if let Some(name) = args.artifact.as_deref() {
        let m = row
            .artifacts
            .iter()
            .find(|a| a.filename == name)
            .ok_or_else(|| anyhow!("no artifact named `{name}` on model {}", row.id))?;
        targets.push(m);
    } else {
        targets.extend(row.artifacts.iter());
    }
    if targets.is_empty() {
        return Err(anyhow!("model has no artifacts to download"));
    }

    let total_bytes: u64 = targets.iter().map(|a| a.byte_length as u64).sum();
    let bar = ProgressBar::new("downloading", total_bytes);
    for a in &targets {
        let dest = out_dir.join(&a.filename);
        if dest.exists() && !args.force {
            eprintln!(
                "{} {} (already on disk; --force to re-download)",
                style::dim("skipped"),
                a.filename
            );
            bar.add(a.byte_length as u64);
            continue;
        }
        let mut f = fs::File::create(&dest)
            .await
            .with_context(|| format!("creating {}", dest.display()))?;
        let bytes = client
            .get_stream_to(
                &format!("/api/models/{}/download/{}", row.id, a.filename),
                &mut f,
            )
            .await?;
        bar.add(bytes);
    }
    bar.finish();
    println!("{}", out_dir.display());
    Ok(())
}

// ── delete ──────────────────────────────────────────────────────────────

async fn delete(client: &Client, args: DeleteArgs) -> Result<()> {
    if !args.yes {
        eprintln!(
            "About to soft-delete model {}. The artifacts remain in R2 until the cleanup sweep purges them (~30 days).",
            args.model_id,
        );
        eprintln!("Re-run with --yes to confirm.");
        return Err(anyhow!("aborted"));
    }
    client
        .delete(&format!("/api/models/{}", args.model_id))
        .await?;
    println!("{} {}", style::bold("Deleted"), style::dim(&args.model_id));
    Ok(())
}

// ── helpers ────────────────────────────────────────────────────────────

fn truncate(s: &str, n: usize) -> &str {
    if s.len() > n {
        &s[..n]
    } else {
        s
    }
}

fn fmt_metric(n: Option<f64>) -> String {
    match n {
        Some(v) => format!("{v:.3}"),
        None => "—".to_string(),
    }
}

fn human_bytes(n: i64) -> String {
    let f = n as f64;
    if f < 1024.0 {
        format!("{n} B")
    } else if f < 1024.0 * 1024.0 {
        format!("{:.1} KB", f / 1024.0)
    } else if f < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", f / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", f / (1024.0 * 1024.0 * 1024.0))
    }
}
