# CLAUDE.md

Guidance for Claude (and any agent editing this repo) on conventions
specific to `wavekat-cli`.

## Adding, removing, or changing a `wk` command

The CLI surface is documented in **multiple places** that all have to
stay in sync. When you add a new subcommand, rename a flag, change a
`--json` shape, or remove a command, walk this list end-to-end before
declaring the change done:

1. **`src/main.rs`** — register/unregister the variant in the `Command`
   enum, add the dispatch arm in `main()`, and update the top-level
   `long_about` if the change affects the bootstrap story (auth,
   self-update, agents guide).
2. **`src/commands.rs`** — add/remove the `pub mod` line.
3. **`src/commands/<name>.rs`** — the command itself. Every read
   command should support `--json` (see existing commands for the
   shape pattern: emit a stable JSON document, never the styled
   table).
4. **`README.md`**:
   - the **What you can do today** command table near the top
   - the **API reference** table at the bottom (command → endpoint)
   - any walkthrough section the command participates in (e.g. the
     **Datasets — end-to-end** flow uses `exports create` →
     `exports download` → `exports adapt smart-turn`)
5. **`AGENTS.md`**:
   - the **Output contract** `--json` shape table
   - the **Recipes** section if the new command unlocks a useful
     workflow
   - the **Quirks worth knowing** list if the command has surprising
     timing/concurrency/limits an agent should know about
6. **`CHANGELOG.md`** — release-plz writes the entry on the release PR;
   you don't edit it directly, but make sure the conventional-commit
   subject (`feat:`, `fix:`, `chore:`) is correct so the release notes
   pick the right section.

If the change is purely internal (refactor, perf, dependency bump
that doesn't move flags or output), only steps 1–3 apply. **Anything
user-visible needs README + AGENTS updates** — they're the contract.

## Releases

- `Cargo.toml` `version` is bumped by release-plz on its release PR;
  don't bump it manually.
- The release pipeline is **draft → atomic promote**: release-plz
  publishes a draft GitHub release, `.github/workflows/release.yml`
  builds binaries + uploads `install.sh` into the draft, then
  promotes via `gh release edit --draft=false --latest`. This means
  `/releases/latest` never points at a tag whose artifacts are still
  building — both fresh `curl | sh` installs and `wk update` see a
  complete release at all times.
- If you touch `release.yml` or `release-plz.toml`, preserve that
  invariant. Specifically: don't remove the `publish` job, and don't
  flip `git_release_draft` back to `false` in `release-plz.toml`.

## Commit style

Conventional Commits (`feat:`, `fix:`, `chore:`, `docs:`, `refactor:`,
`ci:`, `test:`). PR titles must also be conventional and stay under
~50 characters; put detail in the body, not the title.

## Where to look for what

| Topic                              | File                                                  |
|------------------------------------|-------------------------------------------------------|
| HTTP client, auth-header injection | `src/client.rs`                                       |
| Auth file location, schema         | `src/config.rs`                                       |
| ANSI styling, NO_COLOR / IsTerminal | `src/style.rs`                                        |
| Spinner / progress UX              | `src/progress.rs`                                     |
| Per-command logic                  | `src/commands/<name>.rs`                              |
| LLM-facing contract                | `AGENTS.md`                                           |
| Human install / examples           | `README.md`                                           |
| Release pipeline                   | `.github/workflows/release.yml`, `release-plz.toml`   |
| Installer (curl \| sh)             | `install.sh`                                          |
