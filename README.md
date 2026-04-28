<p align="center">
  <a href="https://github.com/wavekat/wavekat-cli">
    <img src="https://github.com/wavekat/wavekat-brand/raw/main/assets/banners/wavekat-cli-narrow.svg" alt="WaveKat CLI">
  </a>
</p>

[![Crates.io](https://img.shields.io/crates/v/wavekat-cli.svg)](https://crates.io/crates/wavekat-cli)

Command-line client (`wk`) for the [WaveKat platform](https://platform.wavekat.com).

> [!NOTE]
> Early development. `wk login` now uses a browser-based loopback OAuth
> handshake (no more pasting cookies). Export commands are still pending.

## What it does today

| Command | What it calls | What it shows |
|---------|---------------|---------------|
| `wk login`               | loopback OAuth + `GET /api/me` | stores `{base_url, token}` under your config dir |
| `wk logout`              | `POST /api/auth/cli/tokens/revoke-current` | revokes the token server-side and removes the local file |
| `wk me`                  | `GET /api/me` | your login, id, name, email, role |
| `wk projects list`       | `GET /api/projects` | paginated table of projects you can see |
| `wk projects show <id>`  | `GET /api/projects/{id}` | project summary (`--json` for raw) |
| `wk annotations list <project-id>` | `GET /api/projects/{id}/annotations` | paginated table with inline ASR (`--json` for raw) |

Pagination on every list (`--page`, `--page-size`, default page size 20).
The footer shows the current page and prints a ready-to-paste `Next:`
line when more pages exist. Filters on annotations: `--label`,
`--review-status`, `--file-id`, `--created-by`. Run any command with
`--help` for the full set.

## Install

### curl | sh

```sh
curl -fsSL https://github.com/wavekat/wavekat-cli/releases/latest/download/install.sh | sh
```

Pin a specific version with `WK_VERSION=v0.0.3` or pick the install directory
with `WK_INSTALL_DIR=$HOME/bin`. Defaults to `/usr/local/bin` if writable, else
`$HOME/.local/bin`. Supports macOS (Apple Silicon + Intel) and Linux (x86_64 +
aarch64, statically linked against musl).

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
# (or: wk login --base-url https://platform.wavekat.com)
```

What happens:

1. `wk` binds an ephemeral port on `127.0.0.1`.
2. Your default browser opens to `<platform>/cli-login`. If you're not
   already signed in, you're bounced through the normal "Sign in with
   GitHub" flow first and come back automatically.
3. You click **Authorize** on the platform's confirmation page.
4. The platform redirects the browser to the loopback URL with a freshly
   minted token; `wk` captures it, verifies against `/api/me`, and writes
   it to your config file.
5. The browser tab shows "You can close this tab" and you're done in your
   terminal.

The token is a long-lived `wkcli_…` bearer credential. You can list and
revoke tokens from your platform profile page; `wk logout` revokes the
current token before clearing the local file.

### Headless / SSH

If no browser is available on the local machine, run:

```sh
wk login --no-browser
```

`wk` prints the authorization URL — open it on any browser that can
reach the loopback port (typically with `ssh -L 1234:127.0.0.1:1234
remote-host`, then open the URL the CLI prints).

### CI / pre-minted token

Pre-mint a token from the SPA (or the API), then:

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

## What's next

The next milestone for the CLI is the **dataset export** feature, landing
together with the matching platform changes. It will add:

- `wk exports create` / `list` / `show` / `download`.
- A built-in adapter that materialises the canonical snapshot into the
  HuggingFace `datasets` format Pipecat `smart-turn` consumes.

## License

Apache-2.0. See [LICENSE](LICENSE).
