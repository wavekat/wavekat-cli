//! Convert a downloaded WaveKat export into Parquet shards loadable by
//! HuggingFace `datasets` for Pipecat smart-turn (binary endpoint task).
//!
//! Output layout (all sibling files under `--out`):
//!
//!   train.parquet
//!   validation.parquet      (optional, omitted if the manifest has none)
//!   test.parquet            (optional, ditto)
//!   README.md
//!   wavekat_export_meta.json
//!
//! The trainer loads it with:
//!
//!   ```python
//!   from datasets import load_dataset, Audio
//!   ds = load_dataset(
//!       "parquet",
//!       data_files={"train": "train.parquet", "validation": "validation.parquet",
//!                   "test": "test.parquet"},
//!   ).cast_column("audio", Audio(sampling_rate=16000))
//!   ```
//!
//! Audio is embedded as the HF `Audio` Parquet shape — a struct
//! `{bytes: BINARY, path: STRING}` — so `cast_column` is the only manual
//! step. We keep `path` populated with the original `clips/<id>.wav`
//! reference so the dataset round-trips back to the canonical snapshot
//! cleanly.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use arrow_array::builder::{
    BinaryBuilder, Float64Builder, Int32Builder, Int64Builder, StringBuilder,
};
use arrow_array::{ArrayRef, RecordBatch, StructArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};

const KNOWN_SPLITS: &[&str] = &["train", "validation", "test"];

#[derive(Debug, Clone)]
pub struct AdaptOptions {
    pub manifest_path: PathBuf,
    pub clips_dir: PathBuf,
    pub out_dir: PathBuf,
    pub language: String,
}

#[derive(Debug)]
pub struct AdaptOutcome {
    pub split_counts: BTreeMap<String, usize>,
    pub total: usize,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ManifestRow {
    annotation_id: String,
    clip_path: String,
    clip_sha256: String,
    clip_duration_sec: f64,
    #[allow(dead_code)]
    clip_sample_rate: i32,
    label_key: String,
    #[allow(dead_code)]
    label_value: i64,
    source_file_id: String,
    source_file_sha256: String,
    labeller_id: i64,
    #[allow(dead_code)]
    review_status: String,
    split: String,
}

/// Map a smart-turn `label_key` to the binary `endpoint_bool` field.
/// Anything else is a config error — we refuse to silently collapse a
/// richer label set into binary.
fn label_to_endpoint(key: &str) -> Result<i32> {
    match key {
        "end_of_turn" => Ok(1),
        "continuation" => Ok(0),
        other => Err(anyhow!(
            "label_key {other:?} is not binary smart-turn (expected `end_of_turn` or \
             `continuation`). Refusing to silently collapse — pick a different export filter."
        )),
    }
}

/// HF Audio feature parquet shape: `STRUCT<bytes: BINARY, path: STRING>`.
fn audio_struct_field() -> Field {
    Field::new(
        "audio",
        DataType::Struct(
            vec![
                Field::new("bytes", DataType::Binary, true),
                Field::new("path", DataType::Utf8, true),
            ]
            .into(),
        ),
        false,
    )
}

fn build_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("annotation_id", DataType::Utf8, false),
        audio_struct_field(),
        Field::new("endpoint_bool", DataType::Int32, false),
        Field::new("language", DataType::Utf8, false),
        Field::new("clip_sha256", DataType::Utf8, false),
        Field::new("source_file_id", DataType::Utf8, false),
        Field::new("source_file_sha256", DataType::Utf8, false),
        Field::new("labeller_id", DataType::Int64, false),
        Field::new("clip_duration_sec", DataType::Float64, false),
    ]))
}

async fn load_manifest(path: &std::path::Path) -> Result<Vec<ManifestRow>> {
    let f = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    let mut lines = BufReader::new(f).lines();
    let mut out = Vec::new();
    let mut lineno = 0usize;
    while let Some(line) = lines.next_line().await? {
        lineno += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let row: ManifestRow = serde_json::from_str(trimmed)
            .with_context(|| format!("{}:{lineno}: invalid manifest line", path.display()))?;
        out.push(row);
    }
    Ok(out)
}

fn resolve_clip_path(row: &ManifestRow, clips_dir: &std::path::Path) -> PathBuf {
    // Manifest references clips as `clips/<annotation_id>.wav`; the user
    // already pointed `clips_dir` at that bare directory, so strip the
    // segment when present.
    let rel = row
        .clip_path
        .strip_prefix("clips/")
        .unwrap_or(&row.clip_path);
    clips_dir.join(rel)
}

fn build_record_batch(
    rows: &[ManifestRow],
    clips_dir: &std::path::Path,
    language: &str,
    schema: Arc<Schema>,
) -> Result<RecordBatch> {
    let mut annotation_id = StringBuilder::new();
    let mut audio_bytes = BinaryBuilder::new();
    let mut audio_path = StringBuilder::new();
    let mut endpoint_bool = Int32Builder::new();
    let mut language_b = StringBuilder::new();
    let mut clip_sha256 = StringBuilder::new();
    let mut source_file_id = StringBuilder::new();
    let mut source_file_sha256 = StringBuilder::new();
    let mut labeller_id = Int64Builder::new();
    let mut clip_duration_sec = Float64Builder::new();

    for row in rows {
        let path = resolve_clip_path(row, clips_dir);
        let bytes =
            std::fs::read(&path).with_context(|| format!("reading clip {}", path.display()))?;
        let bool_value = label_to_endpoint(&row.label_key)?;

        annotation_id.append_value(&row.annotation_id);
        audio_bytes.append_value(&bytes);
        audio_path.append_value(&row.clip_path);
        endpoint_bool.append_value(bool_value);
        language_b.append_value(language);
        clip_sha256.append_value(&row.clip_sha256);
        source_file_id.append_value(&row.source_file_id);
        source_file_sha256.append_value(&row.source_file_sha256);
        labeller_id.append_value(row.labeller_id);
        clip_duration_sec.append_value(row.clip_duration_sec);
    }

    // Assemble the audio struct from its two child arrays. `Field`
    // metadata for the children must match the schema's Struct exactly,
    // or the writer rejects the batch with a confusing schema-mismatch.
    let audio_field_children: Vec<(Arc<Field>, ArrayRef)> = vec![
        (
            Arc::new(Field::new("bytes", DataType::Binary, true)),
            Arc::new(audio_bytes.finish()) as ArrayRef,
        ),
        (
            Arc::new(Field::new("path", DataType::Utf8, true)),
            Arc::new(audio_path.finish()) as ArrayRef,
        ),
    ];
    let audio_array = StructArray::from(audio_field_children);

    let columns: Vec<ArrayRef> = vec![
        Arc::new(annotation_id.finish()),
        Arc::new(audio_array),
        Arc::new(endpoint_bool.finish()),
        Arc::new(language_b.finish()),
        Arc::new(clip_sha256.finish()),
        Arc::new(source_file_id.finish()),
        Arc::new(source_file_sha256.finish()),
        Arc::new(labeller_id.finish()),
        Arc::new(clip_duration_sec.finish()),
    ];
    RecordBatch::try_new(schema, columns).context("assembling parquet RecordBatch")
}

fn write_parquet(out: PathBuf, batch: &RecordBatch, schema: Arc<Schema>) -> Result<()> {
    let file = File::create(&out).with_context(|| format!("creating {}", out.display()))?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let mut writer =
        ArrowWriter::try_new(file, schema, Some(props)).context("building parquet ArrowWriter")?;
    writer
        .write(batch)
        .context("writing parquet record batch")?;
    writer.close().context("closing parquet writer")?;
    Ok(())
}

const README_TEMPLATE: &str = r#"# WaveKat smart-turn export

Generated by `wk exports adapt smart-turn`. See WaveKat's
`docs/06-export.md` for the canonical snapshot shape this was produced
from.

## Load

```python
from datasets import load_dataset, Audio

data_files = {"train": "train.parquet"}
# Add validation / test only if those files exist in this directory.
ds = load_dataset("parquet", data_files=data_files)
ds = ds.cast_column("audio", Audio(sampling_rate=16000))
```

## Schema

| column | type | notes |
|--------|------|-------|
| `annotation_id` | string | source platform annotation id |
| `audio` | struct(bytes, path) | HF Audio feature; `cast_column` to decode |
| `endpoint_bool` | int32 | 1 = end_of_turn, 0 = continuation |
| `language` | string | from `--language` |
| `clip_sha256` | string | passthrough from canonical snapshot |
| `source_file_id` | string | passthrough |
| `source_file_sha256` | string | passthrough |
| `labeller_id` | int64 | passthrough |
| `clip_duration_sec` | float64 | passthrough |
"#;

pub async fn run(opts: AdaptOptions) -> Result<AdaptOutcome> {
    if !opts.manifest_path.exists() {
        return Err(anyhow!(
            "{}: manifest not found",
            opts.manifest_path.display()
        ));
    }
    if !opts.clips_dir.exists() {
        return Err(anyhow!(
            "{}: clips directory not found",
            opts.clips_dir.display()
        ));
    }
    tokio::fs::create_dir_all(&opts.out_dir)
        .await
        .with_context(|| format!("creating {}", opts.out_dir.display()))?;

    let rows = load_manifest(&opts.manifest_path).await?;
    if rows.is_empty() {
        return Err(anyhow!(
            "{}: manifest is empty",
            opts.manifest_path.display()
        ));
    }

    // Group rows by split. Reject anything we don't recognise rather than
    // silently dropping — a typo'd split name in the manifest would
    // otherwise vanish from the output without warning.
    let mut by_split: BTreeMap<String, Vec<ManifestRow>> = BTreeMap::new();
    for row in rows.iter() {
        if !KNOWN_SPLITS.contains(&row.split.as_str()) {
            return Err(anyhow!(
                "manifest references unknown split {:?}; expected one of {:?}",
                row.split,
                KNOWN_SPLITS
            ));
        }
        by_split
            .entry(row.split.clone())
            .or_default()
            .push(row.clone());
    }

    let schema = build_schema();
    let mut split_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;
    for (split_name, split_rows) in &by_split {
        let batch =
            build_record_batch(split_rows, &opts.clips_dir, &opts.language, schema.clone())?;
        let out_path = opts.out_dir.join(format!("{split_name}.parquet"));
        write_parquet(out_path, &batch, schema.clone())?;
        split_counts.insert(split_name.clone(), split_rows.len());
        total += split_rows.len();
    }

    // Drop README + provenance file alongside the parquet shards. The
    // provenance JSON is the bit a future debugger will be glad to find:
    // it traces the dataset back to the manifest path that produced it.
    tokio::fs::write(opts.out_dir.join("README.md"), README_TEMPLATE.as_bytes())
        .await
        .context("writing README.md")?;
    let meta = serde_json::json!({
        "tool": "wk exports adapt smart-turn",
        "manifest": opts.manifest_path.to_string_lossy(),
        "clips_dir": opts.clips_dir.to_string_lossy(),
        "language": opts.language,
        "split_counts": split_counts,
    });
    tokio::fs::write(
        opts.out_dir.join("wavekat_export_meta.json"),
        serde_json::to_vec_pretty(&meta).unwrap(),
    )
    .await
    .context("writing wavekat_export_meta.json")?;

    Ok(AdaptOutcome {
        split_counts,
        total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use std::io::Write;

    #[test]
    fn label_mapping_is_binary() {
        assert_eq!(label_to_endpoint("end_of_turn").unwrap(), 1);
        assert_eq!(label_to_endpoint("continuation").unwrap(), 0);
        assert!(label_to_endpoint("speaker_change").is_err());
    }

    #[test]
    fn schema_includes_audio_struct() {
        let s = build_schema();
        let audio = s.field_with_name("audio").unwrap();
        match audio.data_type() {
            DataType::Struct(fields) => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name(), "bytes");
                assert_eq!(fields[1].name(), "path");
            }
            other => panic!("expected struct, got {other:?}"),
        }
    }

    #[test]
    fn resolve_path_handles_clips_prefix() {
        let row = ManifestRow {
            annotation_id: "abc".into(),
            clip_path: "clips/abc.wav".into(),
            clip_sha256: String::new(),
            clip_duration_sec: 0.0,
            clip_sample_rate: 16000,
            label_key: "end_of_turn".into(),
            label_value: 1,
            source_file_id: String::new(),
            source_file_sha256: String::new(),
            labeller_id: 0,
            review_status: "approved".into(),
            split: "train".into(),
        };
        let p = resolve_clip_path(&row, std::path::Path::new("/tmp/c"));
        assert_eq!(p, std::path::PathBuf::from("/tmp/c/abc.wav"));
    }

    /// Build a 6-clip / 3-split fixture export, run the adapter, and read
    /// the resulting parquet back. Catches real writer bugs (schema
    /// drift, struct-array layout, compression) that the unit tests
    /// don't cover.
    #[tokio::test]
    async fn round_trip_writes_readable_parquet() {
        let tmp = tempdir();
        let export_dir = tmp.path().join("export");
        let clips_dir = export_dir.join("clips");
        std::fs::create_dir_all(&clips_dir).unwrap();

        let plan: &[(&str, &str, i64, &str)] = &[
            ("a1", "end_of_turn", 1, "train"),
            ("a2", "end_of_turn", 1, "train"),
            ("a3", "end_of_turn", 1, "train"),
            ("a4", "continuation", 0, "validation"),
            ("a5", "continuation", 0, "test"),
            ("a6", "end_of_turn", 1, "test"),
        ];
        let mut manifest = String::new();
        for (id, key, val, split) in plan {
            // Tiny non-empty payload — we don't actually decode it, only
            // round-trip the bytes.
            std::fs::write(clips_dir.join(format!("{id}.wav")), b"RIFFfakewavbytes").unwrap();
            let line = format!(
                r#"{{"annotationId":"{id}","clipPath":"clips/{id}.wav","clipSha256":"sha","clipDurationSec":1.5,"clipSampleRate":16000,"labelKey":"{key}","labelValue":{val},"startSec":0.0,"endSec":1.5,"padSec":0.0,"sourceFileId":"f0","sourceFileSha256":"s0","labellerId":1,"reviewStatus":"approved","split":"{split}"}}"#,
            );
            manifest.push_str(&line);
            manifest.push('\n');
        }
        let manifest_path = export_dir.join("manifest.jsonl");
        std::fs::File::create(&manifest_path)
            .unwrap()
            .write_all(manifest.as_bytes())
            .unwrap();

        let out = tmp.path().join("out");
        let outcome = run(AdaptOptions {
            manifest_path: manifest_path.clone(),
            clips_dir: clips_dir.clone(),
            out_dir: out.clone(),
            language: "zh".into(),
        })
        .await
        .expect("adapter run");

        assert_eq!(outcome.total, 6);
        assert_eq!(outcome.split_counts["train"], 3);
        assert_eq!(outcome.split_counts["validation"], 1);
        assert_eq!(outcome.split_counts["test"], 2);

        let train = out.join("train.parquet");
        assert!(train.exists());
        let f = std::fs::File::open(&train).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(f)
            .unwrap()
            .build()
            .unwrap();
        let mut total_rows = 0;
        for batch in reader {
            let batch = batch.unwrap();
            total_rows += batch.num_rows();
            // Audio must round-trip as a struct with `bytes` and `path`.
            let audio = batch
                .column_by_name("audio")
                .expect("audio column")
                .as_any()
                .downcast_ref::<arrow_array::StructArray>()
                .expect("audio is a struct");
            assert_eq!(audio.num_columns(), 2);
        }
        assert_eq!(total_rows, 3);

        // README + provenance JSON must land alongside the shards.
        assert!(out.join("README.md").exists());
        assert!(out.join("wavekat_export_meta.json").exists());
    }

    #[tokio::test]
    async fn unknown_label_aborts() {
        let tmp = tempdir();
        let export_dir = tmp.path().join("export");
        let clips_dir = export_dir.join("clips");
        std::fs::create_dir_all(&clips_dir).unwrap();
        std::fs::write(clips_dir.join("a1.wav"), b"x").unwrap();
        let line = r#"{"annotationId":"a1","clipPath":"clips/a1.wav","clipSha256":"s","clipDurationSec":1.0,"clipSampleRate":16000,"labelKey":"speaker_change","labelValue":1,"startSec":0.0,"endSec":1.0,"padSec":0.0,"sourceFileId":"f","sourceFileSha256":"s","labellerId":1,"reviewStatus":"approved","split":"train"}"#;
        std::fs::write(export_dir.join("manifest.jsonl"), format!("{line}\n")).unwrap();

        let err = run(AdaptOptions {
            manifest_path: export_dir.join("manifest.jsonl"),
            clips_dir,
            out_dir: tmp.path().join("out"),
            language: "zh".into(),
        })
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("speaker_change"),
            "unexpected error: {err:#}"
        );
    }

    /// Tiny scoped temp dir so we don't pull in the `tempfile` crate just
    /// for tests.
    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> TempDir {
        let p = std::env::temp_dir().join(format!(
            "wk-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}
