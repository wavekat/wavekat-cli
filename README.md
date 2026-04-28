<p align="center">
  <a href="https://github.com/wavekat/wavekat-cli">
    <img src="https://github.com/wavekat/wavekat-brand/raw/main/assets/banners/wavekat-cli-narrow.svg" alt="WaveKat CLI">
  </a>
</p>

[![Crates.io](https://img.shields.io/crates/v/wavekat-cli.svg)](https://crates.io/crates/wavekat-cli)

Command-line client (`wk`) for the [WaveKat platform](https://platform.wavekat.com).
Sign in once with your browser and inspect projects and annotations from the
terminal.

## Quick start

```sh
curl -fsSL https://github.com/wavekat/wavekat-cli/releases/latest/download/install.sh | sh
wk login
wk projects list
```

That's it — you're signed in and looking at your projects. Run any command
with `--help` to see all flags, or jump to the [Examples](#examples).

## What you can do today

| Command | What it shows |
|---------|---------------|
| `wk login` / `wk logout`                          | sign in via your browser, or sign out |
| `wk me`                                           | who you're signed in as |
| `wk projects list`                                | paginated table of projects you can see |
| `wk projects show <id>`                           | details for one project (`--json` for raw) |
| `wk annotations list <project-id>`                | paginated annotations with inline ASR text |
| `wk exports list <project-id>`                    | exports for a project (status, clip count) |
| `wk exports show <export-id>`                     | one export's filter, split policy, counts |
| `wk exports create <project-id> --name …`         | snapshot the current label set into a frozen export |
| `wk exports download <export-id>`                 | fetch `manifest.jsonl` + every clip into `./<id>/` |
| `wk exports delete <export-id> --yes`             | soft-delete an export (cleanup sweep purges later) |
| `wk exports adapt smart-turn …`                   | convert a downloaded export into HF `datasets` Parquet shards |

Every list command supports `--page` / `--page-size` (default 20) and prints a
ready-to-paste `Next:` line when more pages exist. `wk annotations list`
also takes `--label`, `--review-status`, `--file-id`, and `--created-by`
filters. Add `--json` to any command for machine-readable output you can pipe
into `jq`.

Supported on macOS (Apple Silicon + Intel) and Linux (x86_64 + aarch64).

## Install

### curl | sh (recommended)

```sh
curl -fsSL https://github.com/wavekat/wavekat-cli/releases/latest/download/install.sh | sh
```

Pin a specific version with `WK_VERSION=vX.Y.Z` or pick the install directory
with `WK_INSTALL_DIR=$HOME/bin`. Defaults to `/usr/local/bin` if writable, else
`$HOME/.local/bin`.

### Prebuilt binaries

Each [release](https://github.com/wavekat/wavekat-cli/releases) attaches
tarballs and `.sha256` checksums for the four supported targets — drop the
`wk` binary anywhere on your `PATH`.

### From source

```sh
cargo install --git https://github.com/wavekat/wavekat-cli wavekat-cli
# or, from a clone:
git clone https://github.com/wavekat/wavekat-cli && cd wavekat-cli
cargo install --path .
wk --version
```

## Sign in

```sh
wk login
```

`wk` opens your browser to the WaveKat platform, you click **Authorize**, and
the terminal confirms you're signed in. The browser tab will say "You can
close this tab" when it's done.

You can list and revoke tokens any time from your platform profile page.
`wk logout` revokes the current token before clearing the local file.

### Headless / SSH

If no browser is available locally, run:

```sh
wk login --no-browser
```

`wk` prints a URL — open it on any browser that can reach the loopback port
(typically `ssh -L 1234:127.0.0.1:1234 remote-host`, then open the URL the
CLI prints).

### CI / pre-minted token

Pre-mint a token from your platform profile, then:

```sh
WK_TOKEN='wkcli_…' WK_BASE_URL='https://platform.wavekat.com' wk login
```

### Where credentials are stored

| Platform | Path |
|----------|------|
| macOS    | `~/Library/Application Support/wavekat/auth.json` |
| Linux    | `~/.config/wavekat/auth.json` |
| Windows  | `%APPDATA%\wavekat\auth.json` |

Mode is `0600` on Unix. Run `wk logout` to remove it.

## Examples

```sh
wk me
# login: somebody
# id:    42
# role:  user

wk projects list --page-size 5

wk projects list --json | jq '.projects[].name'

# Default: human-readable table with the ASR snippet under each row.
wk annotations list <project-id> --label end_of_turn --review-status approved

# Pipe raw JSON into jq for scripting.
wk annotations list <project-id> --label end_of_turn --review-status approved --json \
  | jq '.annotations | length'
```

## Datasets — end-to-end

Producing a HuggingFace-loadable training set from a labelled project is
three commands:

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

The smart-turn adapter only knows two label keys — `end_of_turn` (→ 1)
and `continuation` (→ 0). Anything richer is rejected; binary collapse
of a richer label set must be an explicit user decision via the export
filter, not a silent default in the adapter.

## API reference

Each command maps to a single platform endpoint:

| Command | Endpoint |
|---------|----------|
| `wk login`                                  | loopback OAuth + `GET /api/me` |
| `wk logout`                                 | `POST /api/auth/cli/tokens/revoke-current` |
| `wk me`                                     | `GET /api/me` |
| `wk projects list`                          | `GET /api/projects` |
| `wk projects show <id>`                     | `GET /api/projects/{id}` |
| `wk annotations list <project-id>`          | `GET /api/projects/{id}/annotations` |
| `wk exports list <project-id>`              | `GET /api/projects/{id}/exports` |
| `wk exports show <export-id>`               | `GET /api/exports/{id}` |
| `wk exports create <project-id>`            | `POST /api/projects/{id}/exports` |
| `wk exports download <export-id>`           | `GET /api/exports/{id}/manifest` + per-clip `GET /api/exports/{id}/clips/{annotation-id}` |
| `wk exports delete <export-id>`             | `DELETE /api/exports/{id}` |
| `wk exports adapt smart-turn`               | local-only; reads a downloaded snapshot |

## Help and feedback

- `wk --help` (or `wk <command> --help`) for usage details.
- File issues at <https://github.com/wavekat/wavekat-cli/issues>.

## License

Apache-2.0. See [LICENSE](LICENSE).
