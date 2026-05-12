# 01 — Crash and error reporting (`wavekat-cli`)

**Status:** Draft
**Last updated:** 2026-05-13

**Scope.** This doc is the design for client-side crash and error
reporting in `wavekat-cli`. It is written to be **reusable as a
template** — the infrastructure choices (transport, backend, event
schema, scrubbing pipeline) are deliberately not CLI-specific, so
other WaveKat clients can adopt the same pipe later by writing
their own scrubber and choosing their own opt default. Anything in
this doc that is CLI-specific is called out as such.

**This file is public** (the repo is open source). Keep the
discussion at the level of generic engineering concerns. Don't name
or describe other clients.

---

## Problem

`wavekat-cli` is a thick client. It builds requests, validates input,
parses responses, writes files. When something breaks for a user we
currently only see it if:

1. The user notices, and
2. The user files an issue with enough detail to reproduce.

Both bars are high. For a paid product, "silent failure" is the worst
outcome — the user gives up and we never hear about it.

Concretely, the failure classes we are blind to today:

| Class                          | Example                                                              | Server sees it? |
|--------------------------------|----------------------------------------------------------------------|-----------------|
| Auth missing / expired         | `wk projects list` with no `auth.json`, or stale bearer token        | Only as 401s with no version/command context |
| Wrong payload shape from CLI   | Old CLI sends a field name the server has renamed                     | Yes, as 4xx — but we can't tell *which* CLI version did it |
| Response decode failures       | Server adds a required field; old CLI's serde fails                   | **No** — purely client-side |
| Local I/O                      | Manifest download runs out of disk; export adapt can't read a clip   | **No** |
| Network                        | DNS, TLS, proxy interception                                          | **No** |
| Bugs in `adapt smart-turn`     | Panic, audio decode error, parquet write failure                      | **No** |
| Version skew                   | User on `v0.0.5` hitting an endpoint that moved                       | Partially (User-Agent) |

The `User-Agent: wavekat-cli/<version>` header gives the server
*some* signal on requests that actually reach it, but it's not
queryable as a dataset and it disappears entirely for purely-local
failures.

## Goals

- Detect user-facing failures across the installed CLI fleet without
  waiting for the user to report them.
- Capture **enough context to fix the bug**: CLI version, release
  SHA, OS/arch, command name, structured error category, stack
  trace where available.
- **Never** leak user data: request bodies, response bodies beyond
  a short snippet, file paths, environment variables, argv values,
  auth tokens.
- Stay **non-blocking** — reporting must never slow down a command
  or surface an error to the user.
- Make opting out **a single env var** plus a persistent config
  switch.
- Build it on a foundation that other clients can reuse.

## Non-goals

- Product analytics (which commands are most popular, funnel
  analysis). Different question; separate pipe if we want it later.
- Performance tracing / spans / profiling. The SDK can do it; we
  leave it off in v1.
- Replacing server-side logs.

---

## Proposal

### Use the Sentry envelope protocol; start on Sentry's free tier

The single most important call:

**Do not build a crash pipe from scratch.** Stack-trace capture,
symbolication, dedup, breadcrumbs, release tracking, alerting, SDK
quality across languages — these are a year of work to do badly.
They're a checkbox on any Sentry-compatible service.

Adopt the **Sentry envelope protocol**: the open spec the official
Sentry SDKs speak. Multiple servers implement it (Sentry SaaS, self-
hosted Sentry, [GlitchTip](https://glitchtip.com/) self-hosted or
cloud). Whichever backend we point the SDK at, the client code is
identical — moving between them is a DSN change.

That portability is what lets us start cheap and move later without
client work.

#### Backend choice

Our backend infra is Cloudflare-only (Workers, D1, R2 — no k8s, no
managed Postgres, no long-running containers). That rules out
self-hosting GlitchTip or Sentry on our own infra without taking on
ops burden we've deliberately avoided. The realistic options:

| Option                          | Cost              | Free volume                   | Ops burden | Notes |
|---------------------------------|-------------------|-------------------------------|------------|-------|
| **Sentry SaaS Developer**       | Free              | 5k errors / mo, 1 user        | Zero       | Enough for a quiet CLI |
| **Sentry SaaS Team**            | ~$26/mo           | 50k errors / mo               | Zero       | Upgrade lever |
| **GlitchTip Cloud**             | Free → ~$15/mo    | 1k → 100k events / mo         | Zero       | Same protocol, lower price ceiling |
| **GlitchTip on a $5/mo VPS**    | ~$5–10/mo         | Whatever the box does         | **Real** (Postgres, backups, upgrades, TLS, alerting on the dashboard itself) | Cheapest at scale, but it's a server we'd have to operate |
| Hand-rolled receiver on Workers + D1/R2 | Free      | Workers limits                | Builds a new product       | Loses UI, dedup, symbolication — rejected |

**Decision:** start on **Sentry SaaS Developer (free)**. Move to
Sentry Team or GlitchTip Cloud if/when volume warrants. Only spin
up a self-hosted VPS if cost or vendor concerns force it.

Two reasons this is the right call despite the "third-party SaaS"
flavor:

1. **Scrubbing is client-side.** The `before_send` hook means the
   backend only ever sees redacted events — command name, error
   kind, templated endpoint, version, OS. By contract, no user data
   ships. Whether the receiver is on `wavekat.com` or `sentry.io`
   makes no difference to what's in the event.
2. **The protocol is the lock-in boundary, not the vendor.** If we
   ever want our own backend, we change the DSN to a GlitchTip on
   a VPS. Client code unchanged.

Why this combination of protocol + SaaS-for-now:

- **Cross-language SDKs solved.** `sentry` crate for Rust today;
  `@sentry/*` for any JS/browser/Node/Tauri client we write later;
  iOS/Android SDKs if it comes to that. We never write a client
  SDK ourselves.
- **Zero ops.** Matches the rest of our Cloudflare-only posture.
- **Free at our current volume.** A quiet CLI shouldn't come close
  to 5k errors/month.
- **Reversible.** Sentry envelope is an open spec; if Sentry's
  pricing or terms move against us, GlitchTip Cloud or self-hosted
  GlitchTip on a VPS is a DSN change away.

#### Optional: a Cloudflare Worker as a scrubbing proxy

A pattern worth knowing about even if we don't use it in v1: route
the SDK's DSN at a CF Worker we own, have the Worker apply a
second-layer scrub / rate-limit / drop unwanted event types, then
forward to Sentry SaaS. Costs nothing on CF's free tier, gives us a
second control point, hides the upstream vendor from end users.

We probably don't need this for the CLI v1 — the client-side
scrubber is the load-bearing layer — but it's the natural escape
hatch if we ever want to take more control without operating a
database. Mentioned here so future-us doesn't have to rediscover it.

### Release identity

Every build must bake in:

- `release` — `wavekat-cli@{semver}+{git_sha_short}`, e.g.
  `wavekat-cli@0.0.19+93c6d7b`.
- `environment` — `production` / `staging` / `dev` derived from
  build profile.

A `build.rs` writes `GIT_SHA` to an env var that's consumed by
`env!()` in the binary at compile time. Without this, GlitchTip
can't deduplicate properly and we can't tell which version a crash
came from.

### Symbolication

Rust release builds strip symbols by default. Two options:

- **Cheap:** set `debug = 1` (line tables only) in the release
  profile. Modest binary-size cost, readable stack traces without a
  sym server.
- **Right:** build with full debug info, strip into separate files,
  upload `*.dSYM` / `*.debug` / `*.pdb` to GlitchTip via
  `sentry-cli upload-dif` during release CI.

Start with cheap. Move to right if and when v1 stack traces aren't
informative enough.

### Scrubbing — the part we don't outsource

Sentry SDKs expose a `before_send` hook that runs on every event
before transmission. The CLI must register one that **drops
known-sensitive fields**. Non-negotiable.

CLI scrubber rules (`src/telemetry.rs`):

- Strip any field name matching `/token|cookie|authorization|bearer/i`.
- Strip request bodies entirely. (Sentry breadcrumbs may capture them
  otherwise.)
- Replace concrete URL path segments matching ID patterns
  (`proj_*`, `exp_*`, UUID-like) with `:id` placeholders, so two
  users hitting the same endpoint don't fragment into separate
  issues.
- Cap message snippets at 200 chars.
- Default-disable the SDK's env / argv capture entirely.
- Strip absolute paths under the user's home dir from any captured
  string. Keep the basename only.

The scrubber lives in source in this repo so anyone can audit
exactly what flows.

### Event shape

Sentry/GlitchTip already define the event envelope. We layer **tags**
and **contexts** on top so we can query:

```text
tags:
  client:       "wavekat-cli"
  cli.command:  "exports download"
  http.status:  "422"                       (when applicable)
  endpoint:     "/api/projects/:id/annotations"   (templated, never raw)
  error.kind:   "http_4xx" | "decode" | "network" | "panic" | "local_io" | ...

contexts.runtime:
  rust:   "1.83"

contexts.os:
  name:   "macos" | "linux" | "windows"
  arch:   "aarch64" | "x86_64"

contexts.app:
  release:     "wavekat-cli@0.0.19+93c6d7b"
  install_id:  random UUID stored in auth.json (anonymous per machine)
```

### Error taxonomy

A small fixed set of categories under the `error.kind` tag. The CLI
doesn't have to classify perfectly — `local_other` is a fine catch-
all. The categories exist so we don't have to grep error message
strings by hand to find what's hurting.

| `error.kind`     | When                                                  | Source in code                     |
|------------------|-------------------------------------------------------|------------------------------------|
| `http_4xx`       | Non-success status 400–499                            | `client.rs::decode` and siblings   |
| `http_5xx`       | Non-success status 500–599                            | same                               |
| `http_other`     | Other non-success                                     | same                               |
| `network`        | reqwest transport error (DNS, TLS, connection reset)  | `with_context` site in `client.rs` |
| `decode`         | 2xx body that didn't parse                            | `serde_json::from_str` in `decode` |
| `auth_missing`   | `from_config()` returned "no credentials"             | `client.rs::new`                   |
| `local_io`       | File read/write failure during commands               | `commands/*.rs`                    |
| `panic`          | Panic captured by the global hook                     | `sentry::integrations::panic`      |
| `local_other`    | Any other `anyhow::Error` reaching `main`             | global sink in `main.rs`           |

### Capture points

Two small hooks:

**1. `src/client.rs` refactor.** The existing `decode()` and the
other error-emitting helpers already build the user-facing error
string. Refactor those to produce a `ClientError` struct that
carries `kind`, `endpoint` (templated), `method`, `status`,
`snippet`. The user-facing `Display` formatting stays identical;
we just gain structured access for the SDK call.

The templated `endpoint` field is the only part that needs care:
it must come from a *route constant*, not interpolated:

```rust
pub const PROJECT_ANNOTATIONS: &str = "/api/projects/:id/annotations";
fn project_annotations_url(id: &str) -> String { … }
```

Existing call sites stay one line.

**2. `src/main.rs` wrapper.** Initialize Sentry at startup (with the
panic hook from `sentry::integrations::panic`), wrap the dispatch:

```rust
let started = Instant::now();
let result = run(cmd).await;
telemetry::report(&command_name, started.elapsed(), result.as_ref().err());
result
```

`telemetry::report` adds tags, fires the SDK call (which is itself
non-blocking — the Sentry transport queues and flushes on drop), and
swallows errors silently.

### Non-blocking / never-fails

- The Sentry SDK transport runs on a background thread; the call
  site doesn't await network I/O.
- We set a short flush timeout (e.g. 2s) at process exit. If the
  transport doesn't drain in time, we drop the events.
- Any failure inside `telemetry::report` is silently swallowed.
  Reporting must never surface to the user.

### Rate limits and sampling

SDK-side guards (cheap insurance against a misbehaving build):

- `max_breadcrumbs: 50` (default).
- `traces_sample_rate: 0.0` (no perf traces in v1).
- `before_send` enforces a token bucket per `error.kind` — drop
  events past N/min from the same process.
- GlitchTip server applies a hard ceiling per `install_id`.

### What we deliberately do NOT collect

Hard rules. PRs that add anything in this list get rejected.

- Request bodies and response bodies (beyond a 200-char error
  snippet *for HTTP errors only*, after scrubbing).
- Full URLs containing IDs. Always templated.
- File paths from the user's filesystem.
- Environment variables.
- argv / flag values. Command names only.
- System usernames, real hostnames, MAC addresses, IP addresses.
- Auth tokens, session cookies, API keys, any credential material.
- Anything the user typed into a prompt.

### Opt-out mechanics

- `WK_TELEMETRY=0` per invocation.
- `wk config telemetry off` persists `telemetry: false` to
  `auth.json`.
- First run after we ship this prints a one-line stderr notice with
  a link to a public page that lists what's collected. The notice
  is shown once (gated by a flag in `auth.json`).
- Scrubber source at `src/telemetry.rs` is the audit point. Linked
  from the notice.

Opt-out, not opt-in, because an opt-in rate of ~2% defeats the
purpose. The mitigation is: errors only (no successful-command
beacons in v1), strict scrubbing, single env var to disable,
visible source.

### `bug-report` companion (parallel work)

A local-only `wk doctor` / `wk bug-report` command that bundles the
last N captured events into a redacted zip the user can attach to
an issue, with no network call. Same scrubber, no transport. Covers
the "I don't want network telemetry but I want you to look at my
problem" case.

This is parallel work, not in the critical path of shipping the
network pipe.

---

## Alternatives considered

### Roll our own pipe

The original draft proposed this. Pros: total control, smallest
footprint. Cons: months of work to do badly — stack-trace ingest,
dedup, release tracking, symbolication, retention, SDKs in every
language we ever write a client in. Tractable for the CLI alone if
we only want structured HTTP errors, but doesn't survive the first
panic we want a real stack trace for, and doesn't generalize.

### Self-hosting from day one

Pros: full data sovereignty. Cons: we don't run k8s, Postgres, or
long-running containers anywhere else in our infra; standing up
GlitchTip + Postgres + backups + TLS + upgrade discipline is real
ops work for a team that has avoided it on purpose. With a
client-side scrubber, the data-sovereignty benefit is small — the
backend sees redacted events either way. Revisit if/when volume,
cost, or vendor concerns force the question.

### Local log only

Pros: trivially private. Cons: depends on the user filing an issue
with enough detail — same problem we have today. Worth doing **in
addition** as `wk bug-report`, not instead.

---

## Rollout

**Phase 1 — Foundation:**

1. Create a Sentry organization (free Developer plan). One project
   for `wavekat-cli`. Confirm IP scrubbing is on, environment is
   set, retention is acceptable.
2. Mint a DSN. Store it as a build-time env var
   (`WK_SENTRY_DSN`); the binary's release build embeds it via
   `env!()`. Source builds without the var simply no-op.
3. Publish a public redaction policy page (`wavekat.com/docs/crashes`
   or similar) we can link from the first-run notice and the README.
   Page links to the scrubber source.

If at some point we want our own backend (volume, pricing, control):
spin up GlitchTip on a small VPS or Fly machine, run Postgres next
to it, change the build-time DSN. Client code is unaffected.

**Phase 2 — `wavekat-cli`:**

1. Add `sentry` crate behind a build feature flag
   (`--features telemetry`). Off-by-default for source builds is
   the courtesy default for forkers; release binaries flip it on.
2. Bake `GIT_SHA` and version into `release` via `build.rs`.
3. Wire panic hook + `anyhow` wrapper in `src/main.rs`.
4. Implement the `ClientError` refactor in `src/client.rs`; supply
   templated endpoint as a tag.
5. Implement `before_send` scrubber in `src/telemetry.rs`. Unit-
   test it — scrubber tests are the first thing to review.
6. Add `wk config telemetry on|off` subcommand. Persist in
   `auth.json`. Add `install_id` UUID field to `auth.json`,
   generated on first run if missing.
7. Add the first-run stderr notice (gated by a `telemetry_notice`
   flag in `auth.json`).
8. Update `README.md` and `AGENTS.md` per `CLAUDE.md`'s
   "user-visible change" checklist.
9. Watch one week of real volume before treating dashboards as
   load-bearing.

**Phase 3 — `wk bug-report`:**

Local-only event bundler. Same scrubber, no network. Useful even
before Phase 2 lands, since it exercises the scrubber against
real CLI runs.

**Phase 4 — Other clients (out of scope for this doc):**

Other WaveKat clients can adopt the same backend, schema, and
SDK protocol, writing their own `before_send` scrubber and picking
their own opt default. The shared infrastructure (Phase 1) doesn't
need to change.

---

## Open questions

- **`--features telemetry` default.** Released binary has it on;
  source builds have it off. Confirm before shipping whether we
  want a louder build-time signal for forkers.
- **Symbolication tier.** Start with `debug = 1` line tables, or go
  straight to debug-info upload? Decide once Phase 2 ships and we
  see what real CLI stack traces look like.
- **When to move off Sentry SaaS.** Triggers worth defining now:
  monthly events > free-tier limit for 2 consecutive months *and*
  Sentry Team's price isn't justifiable; or a policy / vendor
  reason we don't have today. Re-evaluate at that point — moving
  is a DSN change.
- **First-run notice copy.** Needs review before shipping. Don't
  ship a default written purely by engineering.
- **`install_id` lifetime.** A UUID in `auth.json` dies on
  `wk logout` if logout wipes the file. That's probably fine — we
  don't need stable cross-session identity beyond a single
  authenticated user — but worth confirming during Phase 2.
