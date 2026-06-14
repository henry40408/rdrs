# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

### Rust (run from repo root)

- Format (CI gate): `cargo fmt --all -- --check` — run `cargo fmt` before committing.
- Lint (CI gate): `cargo clippy -- -D warnings` — warnings fail the build.
- Build: `cargo build` (debug) / `cargo build --release`.
- Test: `cargo nextest run` (the project uses nextest, not `cargo test`).
- Single test: `cargo nextest run <substring>` (e.g. `cargo nextest run test_create_category`).
- Coverage (as CI runs it): `RDRS_FAST_HASH=1 cargo llvm-cov nextest --lcov --output-path lcov.info`.

`RDRS_FAST_HASH=1` swaps Argon2 to minimal-cost params so the auth-heavy test
suite isn't dominated by password hashing. Use it for local test runs; **never**
set it in production.

### E2E (Playwright BDD, run from `e2e/`)

- Install: `npm ci`.
- Run all: `npx playwright test` (CI shards with `--shard=1/3` etc. and `--grep-invert "@skip"`).
- Run one feature: `npx playwright test --grep "<scenario or tag>"`.
- After editing a `.feature` file, regenerate specs: `npx bddgen` (or just re-run `npx playwright test`, which regenerates).
- Regenerate README screenshots: `npm run screenshots` (writes to `../screenshots/`).

**Rebuild before E2E/screenshots:** static assets (CSS, JS) and templates are
embedded into the binary via `include_str!`/`include_bytes!` at compile time, and
the E2E global-setup *skips the build if a binary already exists*. After editing
anything under `static/`, `templates/`, or Rust source, run `cargo build` first
or E2E/screenshots will run against stale assets.

## UI changes require screenshot updates

After any change that alters the rendered UI (`static/css/`, `templates/`, or
`static/js/`), regenerate the affected screenshots on demand and include them in
the same change: `cargo build` then `cd e2e && npm run screenshots`. The four
images under `screenshots/` are referenced by `README.md`; stale screenshots are
treated as part of the change. The generator (`e2e/scripts/screenshots.js`) seeds
demo data and captures the unread list (with reading pane) and the keyboard-help
overlay in both light and dark themes.

## Architecture

Layered Rust web app: **Askama templates → Axum handlers → services → models →
SQLite**. See `ARCHITECTURE.md` for the full directory map; the points below are
the cross-cutting facts that span multiple files.

- **SSR-first.** All logged-in pages are server-rendered with Askama. Mutations
  are HTML form POSTs to action endpoints that respond with a flash + redirect
  (`FlashRedirect`); the small amount of client JS in `static/js/` provides
  progressive enhancement (a `swap()` helper for fragment swaps, chrome custom
  elements like the sidebar / flash / keyboard-help, and passkey ceremonies).
  There is **no frontend build tooling** — vanilla ES modules served via
  `include_str!` are the ceiling; do not introduce bundlers/transpilers.

- **Embedded assets.** Everything (HTML templates, CSS, JS, favicons) compiles
  into the single binary. This is why a rebuild is mandatory before E2E sees UI
  edits (see above) and why deployment is a single static binary.

- **Dual-connection DB pool** (`db/pool.rs`). One write connection and one
  read-only connection (`PRAGMA query_only=ON`) under WAL, with priority
  scheduling so interactive user requests preempt background work. Models expose
  CRUD as associated functions and take `*Params` structs instead of long
  positional argument lists. Schema migrations are versioned via
  `PRAGMA user_version` in `db/schema.rs`.

- **Background feed sync** (`services/background.rs`, `feed_sync.rs`). Feeds are
  distributed across 60 one-minute buckets by URL hash (`feed.bucket` column);
  each minute the scheduler syncs the current bucket, using etag /
  if-modified-since and a `JoinSet` concurrency limit of 4.

- **AI summaries** are async: a request is queued to `summary_worker.rs`, which
  calls Kagi and writes `entry_summary` with a status (pending/processing/
  completed/failed); `summary_cache.rs` and `summary_cleanup.rs` cache and prune.

- **Google Reader API** compatibility lives under `handlers/greader/` so existing
  GReader clients (FeedMe, Read You, etc.) can sync.

- **Security cross-cuts:** Ammonia HTML sanitization + tracking-param/pixel
  removal (`services/sanitize.rs`); HMAC-SHA256 signed image proxy
  (`services/image_proxy.rs`); shared SSRF validation (`utils/url_validation.rs`)
  guarding both the readability fetcher and the proxy; Argon2id password hashing;
  WebAuthn/passkey auth (`auth/webauthn.rs`).
