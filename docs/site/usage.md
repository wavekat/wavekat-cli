---
title: Usage
description: Common workflows — listing, filtering, and building HF-loadable training datasets.
order: 3
---

# Usage

## Listing and filtering

```sh
wk projects list --page-size 5
wk projects list --json | jq '.projects[].name'
```

`wk annotations list` accepts filters that map onto the same query parameters as the platform API:

```sh
wk annotations list <project-id> --label end_of_turn --review-status approved
```

Available filters: `--label`, `--review-status`, `--file-id`, `--created-by`. Combine with `--json` for scripting:

```sh
wk annotations list <project-id> --label end_of_turn --review-status approved --json \
  | jq '.annotations | length'
```

## Datasets — end to end

Producing a HuggingFace-loadable training set from a labelled project is three commands:

```sh
# 1. Snapshot the current labels into a frozen export.
wk exports create <project-id> \
  --name "smart-turn-zh 2026-04-28" \
  --review-status approved \
  --label-key end_of_turn \
  --label-key continuation \
  --split random --seed 42 --ratios 0.8,0.1,0.1

# 2. Download the resulting snapshot (manifest + every clip).
wk exports download <export-id> --out ./snapshots/smart-turn-zh

# 3. Convert it into Parquet shards consumable by HF datasets.
wk exports adapt smart-turn \
  --export-dir ./snapshots/smart-turn-zh \
  --out ./datasets/smart-turn-zh \
  --language zh
```

Then on the trainer side:

```python
from datasets import load_dataset, Audio
ds = load_dataset(
    "parquet",
    data_files={
        "train":      "./datasets/smart-turn-zh/train.parquet",
        "validation": "./datasets/smart-turn-zh/validation.parquet",
        "test":       "./datasets/smart-turn-zh/test.parquet",
    },
).cast_column("audio", Audio(sampling_rate=16000))
```

### Smart-turn adapter rules

The smart-turn adapter only knows two label keys — `end_of_turn` (→ 1) and `continuation` (→ 0). Anything richer is rejected; binary collapse of a richer label set must be an explicit user decision via the export filter, not a silent default in the adapter.

Every clip is decoded, downmixed to mono, resampled to 16 kHz, and re-encoded as 16-bit PCM WAV before being written into the parquet. This both validates the source bytes (corrupt or non-WAV clips fail at adapt time, with the failing path in the error) and guarantees the shards contain a uniform shape downstream notebooks can rely on.

## Pagination

Every list command paginates with `--page` and `--page-size` (default 20). When more pages exist, the CLI prints a ready-to-paste `Next:` line you can run verbatim. Or, in JSON mode, read the `next_page` field.

## Output modes

- **Default** — human-readable table, with an ASR snippet under each annotation row.
- **`--json`** — raw API payload, suitable for piping into `jq` or another tool.
