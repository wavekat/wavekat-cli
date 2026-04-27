# wavekat-cli (`wk`)

Command-line client for the [WaveKat platform](https://github.com/wavekat/wavekat-platform).

> [!WARNING]
> Early development. The auth model is intentionally minimal in v1 (paste a
> session cookie). A proper device-code flow lands together with the export
> feature on the platform side.

## What it does today

| Command | What it calls | What it shows |
|---------|---------------|---------------|
| `wk login`               | `GET /api/me` to verify | stores `{base_url, session_cookie}` under your config dir |
| `wk logout`              | — | removes the stored credentials |
| `wk me`                  | `GET /api/me` | your login, id, name, email, role |
| `wk projects list`       | `GET /api/projects` | paginated table of projects you can see |
| `wk projects show <id>`  | `GET /api/projects/{id}` | project detail (raw JSON) |
| `wk annotations list <project-id>` | `GET /api/projects/{id}/annotations` | paginated annotations (raw JSON) |

Pagination on every list (`--page`, `--page-size`). Filters on annotations:
`--label`, `--review-status`, `--file-id`, `--created-by`. Run any command
with `--help` for the full set.

## Install

### From source

```sh
git clone https://github.com/wavekat/wavekat-cli
cd wavekat-cli
cargo install --path .
# `wk` is now on your PATH
wk --version
```

### Homebrew / curl-pipe-shell

Not yet — these will land once the first tagged release is cut. Releases will
ship prebuilt binaries for macOS (Apple Silicon + Intel) and Linux (x86_64 +
aarch64).

## Sign in (v1: paste-the-cookie)

The platform today only authenticates browser sessions via a signed
`wk_session` cookie set after GitHub OAuth. Until the platform exposes a
CLI-friendly auth flow (planned alongside the dataset export feature), `wk`
reuses your browser session.

```sh
wk login --base-url https://platform.wavekat.com
```

You'll be told to:

1. Open the platform URL in your browser and sign in with GitHub.
2. Open dev tools → Application → Cookies → the platform origin.
3. Copy the value of the `wk_session` cookie and paste it at the prompt
   (input is hidden).

`wk login` calls `/api/me` to verify the cookie before storing it. The cookie
has a 7-day TTL; you'll need to re-run `wk login` after that.

You can also pass it non-interactively:

```sh
WK_BASE_URL=https://platform.wavekat.com WK_SESSION='…' wk login
# or
wk login --base-url … --session …
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

wk annotations list <project-id> --label end_of_turn --review-status approved \
  | jq '.annotations | length'
```

## What's next

The next milestone for the CLI is the **dataset export** feature, landing
together with the matching platform changes. It will add:

- Proper device-code login (no more cookie pasting).
- `wk exports create` / `list` / `show` / `download`.
- A built-in adapter that materialises the canonical snapshot into the
  HuggingFace `datasets` format Pipecat `smart-turn` consumes.

See the platform's [docs/06-export.md](https://github.com/wavekat/wavekat-platform/blob/main/docs/06-export.md)
for the design.

## License

Apache-2.0. See [LICENSE](LICENSE).
