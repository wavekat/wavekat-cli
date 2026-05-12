# wavekat-cli — internal design notes

This folder holds **internal design and architecture notes** for
the `wk` CLI: things we want to think through before writing code,
or record after the fact so future-us (and future agents) understand
*why* the code looks the way it does.

Some docs (e.g. crash/error reporting) describe infrastructure that
is **intentionally designed to be reusable** by other WaveKat
clients later — those docs say so at the top, but the design and
the examples stay CLI-focused. This folder is public; keep it
focused on the CLI.

This is **not** the user-facing documentation. Publishable docs live in
[`docs/site/`](./site/) and are served from
[wavekat.com/docs](https://wavekat.com/docs).

## Index

| #   | Title                                                | Status   |
|-----|------------------------------------------------------|----------|
| 01  | [Crash and error reporting (`wavekat-cli`)](./01-crash-and-error-reporting.md) | Draft    |

## Conventions

- New design docs get the next two-digit prefix: `02-…`, `03-…`, etc.
- Add a row to the table above when you add a doc.
- Mark `Status` as **Draft** (still being discussed), **Accepted**
  (we've decided to do this), **Shipped** (code is live), or
  **Superseded** (with a link to whatever replaced it).
- Design docs can be long and exploratory — they're the place to
  capture tradeoffs, not the place to maintain reference material.
  Once a decision ships, the *contract* lives in `README.md` /
  `AGENTS.md` / `docs/site/`, and the design doc just records the
  reasoning.
