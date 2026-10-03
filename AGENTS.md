# AGENTS.md — using `wk` from an LLM-driven agent

This file is for **AI agents** (Claude, GPT, Cursor, code assistants) and
the humans orchestrating them. If you're generating shell calls to `wk`
from a model, this is the contract — follow it and the surface stays
predictable.

You can also read this guide directly from an installed binary:

```sh
wk agents
```

Human-facing docs live at <https://wavekat.com/docs/cli/>.

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
export WK_TOKEN='wk_…'                          # required
export WK_BASE_URL='https://platform.wavekat.com'  # optional; this is the default
wk login                                            # verifies + persists the token
```

After `wk login` succeeds, the token is saved to disk (`~/.config/wavekat/auth.json`
on Linux, `~/Library/Application Support/wavekat/auth.json` on macOS, mode `0600`).
Subsequent commands read it from disk; you don't need to keep `WK_TOKEN`
exported.

For interactive use only (a real human at a real keyboard):

```sh
wk login              # opens a browser (prints a URL instead over SSH / no display)
wk login --no-browser # always print the URL; user pastes a one-time code back
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
| `wk files list <project-id> --json`         | `files`, `page`, `pageSize`, `total`, `totalPages`           |
| `wk files reserve <id> [<id>…] --json`      | array of `{id, …}` rows (one per file) on success            |
| `wk files unreserve <id> [<id>…] --json`    | array of `{id, ok\|error}` rows                              |
| `wk files summary <project-id> --json`      | `{fileCount, annotationCount, labelledSeconds}`              |
| `wk models list <project-id> --json`        | `models`, `page`, `pageSize`, `total`, `totalPages`          |
| `wk models show <id> --json`                | full model row (lineage, metrics, artifacts list)            |
| `wk models push … --json`                   | the finalized model row (or existing row if idempotent)      |

`wk admin …` and `wk api <path>` always print JSON (there is no table
view; `--json` is accepted and ignored). Their output is the platform
endpoint's response body **unchanged**, so its shape is whatever the
endpoint returns — read it from `wk admin spec` (the OpenAPI document)
rather than from this table. Two exceptions, both from the fleet-tag
commands: `installs tags` / `users tags` print `{assignments: […]}`
(removed ones included, `removedAt` set), and `installs untag` /
`users untag` print `{ok: true, removed: <bool>}`.

Local file producers (`wk exports download`, `wk exports adapt smart-turn`)
write files to disk and print the output path on stdout. Progress goes
to stderr.

`--json` is a pass-through — every field the platform returns on a list
row is visible. Useful fields the human table also surfaces (so an agent
can target the same signal):

| List                       | Per-row fields worth filtering on                                                                   |
|----------------------------|-----------------------------------------------------------------------------------------------------|
| `projects list`            | `myRoleInProject`, `filesCount`, `annotationsCount`, `annotationsReviewedCount`, `updatedAt`        |
| `files list`               | `annotationCount`, `labelCounts` (per-key), `labelledSeconds`, `sampleRate`, `testReservedAt`       |
| `exports list`             | `status`, `clipCount`, `clipsTotal` / `clipsWritten` (writer progress), `totalBytes`, `splitCounts` |
| `models list`              | `valF1`, `valThreshold`, `testF1`, `testF1Ci95Low/High`, `testAp`, `createdByLogin`, `status`       |

## Exit codes

- `0` — success
- non-zero — error; a single-line message goes to stderr (anyhow-style
  context chain). There are no fine-grained codes. To distinguish
  "command failed" from "command succeeded but returned an empty list",
  rely on the exit status and the JSON document on stdout — never on
  parsing stderr.
- When a request never reaches the server (DNS failure, refused
  connection, timeout), the first line reads `couldn't look up <host>
  (DNS failed) — check your network connection …` (or `couldn't connect
  to` / `timed out reaching`). That's a local network problem, not a
  `wk` bug — retry once connectivity is back.

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

### Push a trained model after a lab run

```sh
# Drop the FP32 + INT8 ONNX checkpoints and the run's results.json
# into the registry. The push is idempotent on
# (training-export, recipe, sha256(model.onnx)) — re-running with the
# same files exits 0 without re-uploading.
wk models push \
  --project "$PROJECT_ID" \
  --training-export "$EXPORT_ID" \
  --recipe specaugment \
  --results ./checkpoints/specaugment/results.json \
  --artifact ./checkpoints/specaugment/onnx/model.onnx \
  --artifact ./checkpoints/specaugment/onnx/model.int8.onnx \
  --name "smart-turn-zh 0504-specaug" \
  --json | jq -r .id
```

### List every model trained on a given export

```sh
wk models list "$PROJECT_ID" --training-export "$EXPORT_ID" --json \
  | jq '.models[] | {id, name, recipeName, valF1, testF1}'
```

### Download a specific INT8 ONNX

```sh
wk models download "$MODEL_ID" --artifact model.int8.onnx --out ./
```

### Find every annotation that needs review

```sh
wk annotations list "$PROJECT_ID" \
  --review-status needs_fix --review-status unreviewed \
  --json
```

### Reserve a stable held-out test set

The platform supports per-file test-set reservation
(see `docs/08-test-set-reservation.md` in `wavekat-platform`). Reserved
files get pinned to the `test` split on every export so the holdout
stays stable across reshuffles. Owner / `root` only.

```sh
# Mark a curated batch of files as the test set.
wk files reserve "$FILE_ID_A" "$FILE_ID_B" "$FILE_ID_C"

# Inspect the reservation surface for a project.
wk files summary "$PROJECT_ID" --json
wk files list "$PROJECT_ID" --test-reserved true --json

# Export with the reserved files as the test split. Note the 2-tuple
# `--ratios` — the third slot is implicit 0 because `test` is filled
# from reserved files only.
wk exports create "$PROJECT_ID" \
  --name "snapshot $(date -I)" \
  --review-status approved \
  --use-reserved-test-files \
  --split random --seed 42 --ratios 0.9,0.1 \
  --json
```

### Analyse customers and product usage (root accounts only)

Every `wk admin` read is a GET against a root-only endpoint (the
fleet-tag writes are covered in the next recipe). Start from the spec so
you know what each endpoint returns and accepts:

```sh
wk admin spec | jq '.paths | keys[] | select(startswith("/api/admin") or startswith("/api/users"))'
wk admin spec | jq '.paths["/api/admin/voice/installs"].get.parameters'
```

Then pull data, passing filters as `-q key=value`:

```sh
wk admin users list -q pageSize=100 -q sort=recent | jq '.users[] | {id, login, tier}'
wk admin installs metrics -q days=30
wk admin usage | jq '.byName[] | {name, installs7d, installsAllTime}'
wk admin geo -q days=90 | jq '.countries[:10]'
```

Page-paginated endpoints take `-q page=N -q pageSize=N` and report
`totalPages`; cursor-paginated ones (`installs events`, `prompts
events`) take `-q cursor=<nextCursor>` until `nextCursor` is `null`.

Anything without a named command is one `wk api` call away:

```sh
wk api /api/admin/voice/downloads -q pageSize=50
```

### Review an install and record what you found (root accounts only)

Read the install's story, decide what happened, and record it as tags:
a status tag (`reviewed`) plus a cause tag naming the finding. Reuse an
existing cause tag when one fits — tag counts are how findings become
priorities, so near-duplicate tags split the count.

```sh
wk admin installs story <install-id>                 # events + tag changes, by day
wk admin installs events <install-id> -q limit=200   # the raw events, if you need detail
wk admin tags list | jq '.tags[] | {name, description, installs}'
wk admin tags create <cause> --color red --description "<what it means>"
wk admin installs tag <install-id> <cause>
wk admin installs tag <install-id> reviewed
```

To find installs nobody has reviewed yet, page through `wk admin
installs list` and check each one's tags with `wk admin installs tags
<install-id>`.

## Quirks worth knowing

- **`wk login` runs a loopback OAuth handshake.** Don't try to script
  it without `WK_TOKEN`; there is no headless-browser fallback. Over SSH
  it prints the URL and waits on a `Code:` stdin prompt for the one-time
  code the sign-in page shows after the user authorizes.
- **`wk exports create` blocks** until the platform finishes copying
  clips to R2. Seconds-to-minutes is normal. Exit status reflects
  success/failure of the whole operation, not just submission.
- **`wk exports download` fetches clips in parallel** (default 8
  concurrent). Tune with `--concurrency N`; cranking past ~16 hits
  Worker subrequest budgets without meaningfully improving wall time.
  The bar tracks every manifest entry — already-on-disk clips count
  toward progress, so resumes look fast.
- **`wk admin` needs the global `root` role.** Any other token gets a
  `403`, reported as `forbidden — wk admin needs … root role`. There is
  no client-side check; the platform decides.
- **`wk api` is GET-only.** It will not create, change or delete
  anything; mutations stay behind dedicated commands.
- **The fleet-tag commands are the only `wk admin` writes, and all of
  them are safe to retry.** `tags create` on an existing name prints the
  existing tag; `installs tag` on a tag already present returns the
  existing assignment; `installs untag` on a missing tag reports
  `removed: false`. Install commands accept either the row id or the
  app's install id and refuse an id that matches no install. There's no
  free-text note on purpose — record findings as tags.
- **Errors from `wk admin` / `wk api` are not sent to crash reporting**,
  because their URLs and bodies can identify customers.
- **All list endpoints paginate.** Default `--page-size` is 20. Use
  `total` / `totalPages` to know when to stop.
- **The smart-turn adapter only handles two label keys** (`end_of_turn`
  → 1, `continuation` → 0). Richer label sets must be collapsed at
  export time via the `--label-key` filter, not silently in the
  adapter.
- **The smart-turn adapter canonicalises audio.** Every clip is decoded,
  downmixed to mono, resampled to 16 kHz, and re-encoded as 16-bit PCM
  WAV before landing in the parquet. A clip that won't decode aborts
  the run with the failing path in the error — agents can treat such
  errors as a manifest/clip integrity issue, not an adapter bug.
- **Crash reporting is on by default** in the shipped binary. The CLI
  sends anonymous error events (version, OS, subcommand, templated
  endpoint, error category) to Sentry. Request bodies, response
  bodies, file paths, tokens, and argv values are never sent — see
  the scrubber in `src/telemetry.rs`. Disable for a run with
  `WK_TELEMETRY=0`, or persistently with `wk config telemetry off`.
  Agents running in a CI sandbox where you don't want any network
  side effect beyond the platform call should set `WK_TELEMETRY=0`.

## Reporting problems

If `--json` shapes look inconsistent, a flag is missing, or `wk` is
misbehaving in a way that breaks agent use specifically, open an
issue at <https://github.com/wavekat/wavekat-cli/issues> and mention
that the report comes from agent integration.
