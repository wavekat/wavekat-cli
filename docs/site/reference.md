---
title: Reference
description: Every command, environment variable, and the API endpoint each command hits.
order: 4
---

# Reference

## Global flags

All commands accept:

| Flag | Description |
|------|-------------|
| `--json` | Emit raw JSON instead of the human-readable table. |
| `--help` | Show usage for the current command. |

List commands additionally accept:

| Flag | Description |
|------|-------------|
| `--page <n>` | Page index (1-based). |
| `--page-size <n>` | Items per page. Default 20. |

## Environment variables

| Variable | Used for |
|----------|----------|
| `WK_VERSION` | Pin a version when running the install script. |
| `WK_INSTALL_DIR` | Override the install script's target directory. |
| `WK_TOKEN` | Pre-minted CLI token (skip the browser login). |
| `WK_BASE_URL` | Override the platform base URL (defaults to `https://platform.wavekat.com`). |

## Commands → endpoints

Each command maps to a single platform endpoint:

| Command | Endpoint |
|---------|----------|
| `wk login`                          | Loopback OAuth + `GET /api/me` |
| `wk logout`                         | `POST /api/auth/cli/tokens/revoke-current` |
| `wk me`                             | `GET /api/me` |
| `wk projects list`                  | `GET /api/projects` |
| `wk projects show <id>`             | `GET /api/projects/{id}` |
| `wk annotations list <project-id>`  | `GET /api/projects/{id}/annotations` |
| `wk exports list <project-id>`      | `GET /api/projects/{id}/exports` |
| `wk exports show <export-id>`       | `GET /api/exports/{id}` |
| `wk exports create <project-id>`    | `POST /api/projects/{id}/exports` |
| `wk exports download <export-id>`   | `GET /api/exports/{id}/manifest` + per-clip `GET /api/exports/{id}/clips/{annotation-id}` |
| `wk exports delete <export-id>`     | `DELETE /api/exports/{id}` |
| `wk exports adapt smart-turn`       | Local-only; reads a downloaded snapshot |

## Credentials file

Stored at:

| Platform | Path |
|----------|------|
| macOS    | `~/Library/Application Support/wavekat/auth.json` |
| Linux    | `~/.config/wavekat/auth.json` |
| Windows  | `%APPDATA%\wavekat\auth.json` |

Mode is `0600` on Unix. The file contains the active CLI token; `wk logout` revokes it server-side and then removes the file.

## License

[Apache-2.0](https://github.com/wavekat/wavekat-cli/blob/main/LICENSE).

## Issues

File issues at <https://github.com/wavekat/wavekat-cli/issues>.
