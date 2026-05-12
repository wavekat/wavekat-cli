---
title: WaveKat CLI
description: Command-line client (wk) for the WaveKat platform — sign in once, inspect projects, build datasets.
order: 1
---

# WaveKat CLI

`wk` is the command-line client for the [WaveKat platform](https://platform.wavekat.com). Sign in once with your browser, then inspect projects, list annotations, snapshot exports, and adapt them into HuggingFace-loadable datasets — all from the terminal.

## What you can do today

| Command | What it shows |
|---------|---------------|
| `wk login` / `wk logout`              | Sign in via your browser, or sign out |
| `wk projects list`                    | Projects you can see, with role, counts, and review progress |
| `wk annotations list <project-id>`    | Paginated annotations with inline ASR text |
| `wk exports create <project-id>`      | Snapshot the current label set into a frozen export |
| `wk exports download <export-id>`     | Fetch manifest + every clip |
| `wk exports adapt smart-turn …`       | Convert a downloaded export into HF `datasets` Parquet shards |
| `wk models list <project-id>`         | Models trained in a project (lineage, metrics, status) |
| `wk models push …`                    | Register a trained model + upload its artifacts |

Every list command supports `--page` / `--page-size` and prints a ready-to-paste `Next:` line when more pages exist. Add `--json` to any command for machine-readable output.

This is the highlight reel — see [Reference](reference.md) for the full surface (files, model downloads, self-update, etc.), or just run `wk --help`, `wk <group> --help`, or `wk <group> <command> --help` against the binary. Clap-generated help is the authoritative source.

Supported on macOS (Apple Silicon + Intel) and Linux (x86_64 + aarch64).

## Where to go next

- **[Getting Started](getting-started.md)** — install, sign in, list your first project.
- **[Usage](usage.md)** — common workflows including the end-to-end dataset pipeline.
- **[Reference](reference.md)** — every command, every flag, every endpoint.
