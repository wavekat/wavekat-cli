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
| `wk me`                               | Who you're signed in as |
| `wk projects list`                    | Paginated table of projects you can see |
| `wk projects show <id>`               | Details for one project |
| `wk annotations list <project-id>`    | Paginated annotations with inline ASR text |
| `wk exports list <project-id>`        | Exports for a project |
| `wk exports show <export-id>`         | Filter, split policy, counts for one export |
| `wk exports create <project-id>`      | Snapshot the current label set into a frozen export |
| `wk exports download <export-id>`     | Fetch manifest + every clip |
| `wk exports delete <export-id>`       | Soft-delete an export |
| `wk exports adapt smart-turn …`       | Convert a downloaded export into HF `datasets` Parquet shards |

Every list command supports `--page` / `--page-size` and prints a ready-to-paste `Next:` line when more pages exist. Add `--json` to any command for machine-readable output.

Supported on macOS (Apple Silicon + Intel) and Linux (x86_64 + aarch64).

## Where to go next

- **[Getting Started](getting-started.md)** — install, sign in, list your first project.
- **[Usage](usage.md)** — common workflows including the end-to-end dataset pipeline.
- **[Reference](reference.md)** — every command, every flag, every endpoint.
