# AGENTS.md — using `wk` from an LLM-driven agent

This file is for **AI agents** (Claude, GPT, Cursor, code assistants) and
the humans orchestrating them. If you're generating shell calls to `wk`
from a model, this is the contract — follow it and the surface stays
predictable.

You can also read this guide directly from an installed binary:

```sh
wk agents
```

## Install

```sh
curl -fsSL https://github.com/wavekat/wavekat-cli/releases/latest/download/install.sh | sh
```

Verify:

```sh
wk --version
wk version --json     # also probes /api/health on the platform
```

Supported targets: macOS (arm64, x86_64), Linux (x86_64, aarch64; musl-static).

## Authentication

Two paths. **Pre-minted token is the right one for non-interactive agents.**

```sh
export WK_TOKEN='wkcli_…'                          # required
export WK_BASE_URL='https://platform.wavekat.com'  # optional; this is the default
wk login                                            # verifies + persists the token
```

After `wk login` succeeds, the token is saved to disk (`~/.config/wavekat/auth.json`
on Linux, `~/Library/Application Support/wavekat/auth.json` on macOS, mode `0600`).
Subsequent commands read it from disk; you don't need to keep `WK_TOKEN`
exported.

For interactive use only (a real human at a real keyboard):

```sh
wk login              # opens a browser
wk login --no-browser # prints a URL the user opens manually (e.g. SSH)
```

`wk logout` revokes the current token and clears the local file.

## Output contract

Every read command takes `--json`. Without it you get a styled human
table — **do not parse the non-JSON output**, it includes ANSI codes
and the layout is not stable.

| Command                                     | `--json` shape (top-level keys)                              |
|---------------------------------------------|--------------------------------------------------------------|
| `wk version --json`                         | `cli`, `api`, `endpoint`                                     |
| `wk projects list --json`                   | `projects`, `page`, `pageSize`, `total`, `totalPages`        |
| `wk projects show <id> --json`              | full project row                                             |
| `wk annotations list <project-id> --json`   | `annotations`, `page`, `pageSize`, `total`, `totalPages`     |
| `wk exports list <project-id> --json`       | `exports`, `page`, `pageSize`, `total`, `totalPages`         |
| `wk exports show <id> --json`               | full export row                                              |
| `wk exports create … --json`                | the newly created export row (includes `id`, `status`)       |

Local file producers (`wk exports download`, `wk exports adapt smart-turn`)
write files to disk and print the output path on stdout. Progress goes
to stderr.

## Exit codes

- `0` — success
- non-zero — error; a single-line message goes to stderr (anyhow-style
  context chain). There are no fine-grained codes. To distinguish
  "command failed" from "command succeeded but returned an empty list",
  rely on the exit status and the JSON document on stdout — never on
  parsing stderr.

## Self-update

```sh
wk update --check           # is a newer release out?
wk update                   # download + replace this binary
wk update --version v0.0.7  # pin a specific tag
```

`wk update` reuses the official `install.sh` and writes to the same
directory the running binary lives in.

## Discovery

`wk` is built on clap; every subcommand has self-describing help. A
model that can run shell commands can explore the full surface
without external docs:

```sh
wk --help                   # top-level
wk exports --help           # one subcommand group
wk exports create --help    # all flags, types, defaults
```

When in doubt, run `--help` rather than guessing flags.

## Recipes

### Confirm auth is wired up

```sh
wk me --json    # exits non-zero if not signed in
```

### List every project the current user can see

```sh
wk projects list --json | jq '.projects[] | {id, name}'
```

### Snapshot a labelled project into a HuggingFace-loadable dataset

```sh
EXPORT_ID=$(
  wk exports create "$PROJECT_ID" \
    --name "snapshot $(date -I)" \
    --review-status approved \
    --label-key end_of_turn \
    --label-key continuation \
    --split random --seed 42 --ratios 0.8,0.1,0.1 \
    --json | jq -r .id
)
wk exports download "$EXPORT_ID" --out ./snapshot
wk exports adapt smart-turn \
  --export-dir ./snapshot \
  --out ./dataset \
  --language zh
```

### Poll an export until it's ready

```sh
until [ "$(wk exports show "$EXPORT_ID" --json | jq -r .status)" = "ready" ]; do
  sleep 5
done
```

### Find every annotation that needs review

```sh
wk annotations list "$PROJECT_ID" \
  --review-status needs_fix --review-status unreviewed \
  --json
```

## Quirks worth knowing

- **`wk login` runs a loopback OAuth handshake.** Don't try to script
  it without `WK_TOKEN`; there is no headless-browser fallback.
- **`wk exports create` blocks** until the platform finishes copying
  clips to R2. Seconds-to-minutes is normal. Exit status reflects
  success/failure of the whole operation, not just submission.
- **`wk exports download` fetches clips in parallel** (default 8
  concurrent). Tune with `--concurrency N`; cranking past ~16 hits
  Worker subrequest budgets without meaningfully improving wall time.
  The bar tracks every manifest entry — already-on-disk clips count
  toward progress, so resumes look fast.
- **All list endpoints paginate.** Default `--page-size` is 20. Use
  `total` / `totalPages` to know when to stop.
- **The smart-turn adapter only handles two label keys** (`end_of_turn`
  → 1, `continuation` → 0). Richer label sets must be collapsed at
  export time via the `--label-key` filter, not silently in the
  adapter.

## Reporting problems

If `--json` shapes look inconsistent, a flag is missing, or `wk` is
misbehaving in a way that breaks agent use specifically, open an
issue at <https://github.com/wavekat/wavekat-cli/issues> and mention
that the report comes from agent integration.
