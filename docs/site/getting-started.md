---
title: Getting Started
description: Install wk, sign in, and list your first project.
order: 2
---

# Getting Started

## Install

### curl | sh (recommended)

```sh
curl -fsSL https://github.com/wavekat/wavekat-cli/releases/latest/download/install.sh | sh
```

Pin a specific version with `WK_VERSION=vX.Y.Z`, or pick the install directory with `WK_INSTALL_DIR=$HOME/bin`. Defaults to `/usr/local/bin` if writable, else `$HOME/.local/bin`.

### Prebuilt binaries

Each [release](https://github.com/wavekat/wavekat-cli/releases) attaches tarballs and `.sha256` checksums for the four supported targets — drop the `wk` binary anywhere on your `PATH`.

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

`wk` opens your browser to the WaveKat platform, you click **Authorize**, and the terminal confirms you're signed in. The browser tab will say "You can close this tab" when it's done.

You can list and revoke tokens any time from your platform profile page. `wk logout` revokes the current token before clearing the local file.

### Headless / SSH

If no browser is available locally, run:

```sh
wk login --no-browser
```

`wk` prints a URL — open it on any browser that can reach the loopback port (typically `ssh -L 1234:127.0.0.1:1234 remote-host`, then open the URL the CLI prints).

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

## Your first commands

```sh
wk me
# login: somebody
# id:    42
# role:  user

wk projects list --page-size 5
```

That's it — you're signed in and looking at your projects. Run any command with `--help` for all flags, or continue to [Usage](usage.md) for common workflows.
