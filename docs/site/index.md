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
| `wk projects list`                    | Projects you can see, with role, counts, and review progress |
| `wk projects show <id>`               | Details for one project |
| `wk annotations list <project-id>`    | Paginated annotations with inline ASR text |
| `wk files list <project-id>`          | Files in a project, with per-label counts and reservation state |
| `wk files reserve <id> [<id>…]`       | Reserve files for the test set |
| `wk files unreserve <id> [<id>…]`     | Release a test-set reservation |
| `wk files summary <project-id>`       | File / annotation / labelled-seconds totals |
| `wk exports list <project-id>`        | Exports for a project |
| `wk exports show <export-id>`         | Filter, split policy, counts for one export |
| `wk exports create <project-id>`      | Snapshot the current label set into a frozen export |
| `wk exports download <export-id>`     | Fetch manifest + every clip |
| `wk exports delete <export-id>`       | Soft-delete an export |
| `wk exports adapt smart-turn …`       | Convert a downloaded export into HF `datasets` Parquet shards |
| `wk models list <project-id>`         | Models trained in a project (lineage, metrics, status) |
| `wk models show <model-id>`           | Full model row including artifacts |
| `wk models push …`                    | Register a trained model + upload its artifacts |
| `wk models download <model-id>`       | Fetch a specific model artifact |
| `wk version`                          | CLI + API versions, with `--json` |
| `wk update`                           | Self-update to the latest release (or `--check`, `--version`) |
| `wk agents`                           | Print the LLM-facing `AGENTS.md` guide |

Every list command supports `--page` / `--page-size` and prints a ready-to-paste `Next:` line when more pages exist. Add `--json` to any command for machine-readable output.

Don't see what you need above? `wk` is built on clap — every subcommand has self-describing help. Run `wk --help`, `wk <group> --help`, or `wk <group> <command> --help` to see every flag, type, and default for any command this binary supports.

Supported on macOS (Apple Silicon + Intel) and Linux (x86_64 + aarch64).

## Where to go next

- **[Getting Started](getting-started.md)** — install, sign in, list your first project.
- **[Usage](usage.md)** — common workflows including the end-to-end dataset pipeline.
- **[Reference](reference.md)** — every command, every flag, every endpoint.
