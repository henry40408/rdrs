# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

### Rust (run from repo root)

- Format (CI gate): `cargo fmt --all -- --check` — run `cargo fmt` before committing.
- Lint (CI gate): `cargo clippy --all-targets -- -D warnings`.
- Supply-chain (CI gate): `cargo deny check`.
- Build: `cargo build` (debug) / `cargo build --release`.
- Test: `cargo nextest run` (the project uses nextest, not `cargo test`); one
  test: `cargo nextest run <substring>`.
- Coverage (as CI runs it): `RDRS_FAST_HASH=1 cargo llvm-cov nextest --lcov --output-path lcov.info`.

`RDRS_FAST_HASH=1` swaps Argon2 to minimal-cost params so the auth-heavy test
suite isn't dominated by password hashing. Use it for local test runs; **never**
set it in production.

### E2E (Playwright BDD, run from `e2e/`)

- Install: `npm ci`. Run all: `npx playwright test` (CI shards with
  `--shard=1/3` etc. and `--grep-invert "@skip"`). One feature:
  `npx playwright test --grep "<scenario or tag>"`.
- Regenerate README screenshots: `npm run screenshots` (writes to `../screenshots/`).

**After editing a `.feature` file, run `npx bddgen`.** The generated
`.features-gen/` is git-ignored and the project's custom `globalSetup` shadows
playwright-bdd's generation hook, so a fresh checkout without a prior `bddgen`
has no specs: `playwright test` silently runs zero tests and passes. CI runs
`bddgen` as an explicit step for the same reason.

**Rebuild before E2E/screenshots.** Static assets and templates are embedded
into the binary via `include_str!`/`include_bytes!` at compile time, and the E2E
global-setup *skips the build if a binary already exists*. After editing
anything under `static/`, `templates/`, or Rust source, run `cargo build` first
or you are testing stale assets.

## UI changes require screenshot updates

After any change that alters the rendered UI (`static/css/`, `templates/`,
`static/js/`): `cargo build`, then `cd e2e && npm run screenshots`, and include
the result in the same change. The four images under `screenshots/` are
referenced by `README.md`, so stale screenshots count as part of the change. The
generator (`e2e/scripts/screenshots.js`) seeds demo data and captures the unread
list (with reading pane) and the keyboard-help overlay in light and dark themes.

## Architecture

Layered Rust web app: **Askama templates → Axum handlers → services → models →
SQLite/PostgreSQL**. `ARCHITECTURE.md` is the full map — directory structure,
dual-backend data layer, feed sync, AI summaries, GReader API, SSE, security.
The cross-cutting rules most easily violated:

- **SSR-first, and there is no frontend build tooling.** Logged-in pages are
  server-rendered Askama; mutations are HTML form POSTs answered with a flash +
  redirect (`FlashRedirect`). The JS in `static/js/` is progressive enhancement
  only — vanilla ES modules served via `include_str!` are the ceiling; do not
  introduce bundlers or transpilers.
- **Everything compiles into the single binary** (templates, CSS, JS, favicons),
  which is why the rebuild above is mandatory and why deployment is one static
  binary.
- **Every query goes through the `query_*!` / `db_execute!` macros** so SQL and
  binds are written once for both backends. Dialect forks belong in
  `entry::filters::Dialect` or the `pg_rewrite` shim — except the entry upsert's
  NULL-safe inequality (`IS NOT` / `IS DISTINCT FROM`), a hand-dispatched
  `*_SQLITE` / `*_PG` literal pair *on purpose*: `pg_rewrite` substitutes
  blindly and an `IS NOT` rule would corrupt every `IS NOT NULL`. Migrations are
  embedded per backend under `migrations/{sqlite,postgres}/`.
- **Background DB work must call `db.background()`** so it yields to interactive
  work while SQLite's single writer is contended (no-op on PostgreSQL).
- **`RDRS_SECRET` is the one root key**: the session cookie, image proxy, GReader
  post token and CSRF token all derive from it with domain separation in
  `secret.rs`. Session lifecycle events (creation, renewal, destruction,
  masquerade start/stop) are audited under the `rdrs::audit` tracing target,
  identifying sessions only by a salted `secret::audit_id` hash.
