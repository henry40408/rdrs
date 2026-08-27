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

### E2E (cucumber + thirtyfour, run from `e2e/`)

`e2e/` is **its own cargo workspace**, deliberately: a `--workspace` coverage
run at the root would otherwise compile thirtyfour and drive a real browser.
That also means the root's `cargo fmt --all` and `cargo clippy` do not reach it,
so CI lints it as a separate step and so should you.

- Run all: `cargo test --lib --test e2e` — `--lib` picks up the browser-free
  unit tests on the wait helpers, which `--test e2e` alone would compile and
  skip. One feature, while working on it:
  `RDRS_E2E_FEATURES=features/reading.feature cargo test --test e2e`.
- Format / lint (CI gates): `cargo fmt --all -- --check` and
  `cargo clippy --all-targets -- -D warnings`, from `e2e/`.
- Regenerate README screenshots: `cargo run --bin screenshots` (writes to
  `../screenshots/`).
- CSP audit (CI gate): `cargo run --bin csp-audit` — walks the app in a browser
  and fails on any Content Security Policy violation. Run it after touching
  `templates/`, `static/css/` or `static/js/`; the Rust-side scan in
  `src/middleware/security_headers.rs` only greps sources and cannot see
  runtime-injected markup or shadow DOM.
- No-JS walkthrough (CI gate): `cargo run --bin nojs` — drives the app with
  script execution disabled *and* every `*.js` request aborted.
- Touch-target report (not a gate): `cargo run --bin touch-audit`.
- Swap benchmark: `cargo run --bin dom-bench -- --label before` (`--entries`,
  `--rows`, `--mode`, `--profile`; see the module docs). This is what the
  "capture a baseline before and after" rule for performance work measures.

**A browser must be installed**, unlike under Playwright: `WebDriver::managed`
downloads and supervises the *driver*, never the browser. On macOS,
`brew install --cask ungoogled-chromium`; CI's `ubuntu-latest` already ships
Chrome.

**Rebuild before E2E/screenshots.** Static assets and templates are embedded
into the binary via `include_str!`/`include_bytes!` at compile time, and the
suite *skips the build if a binary already exists*. After editing anything
under `static/`, `templates/`, or Rust source, run `cargo build` first or you
are testing stale assets.

**The `.feature` files are the contract** and are read as-is — there is no
generation step. Adding a scenario means adding steps under
`e2e/tests/e2e/steps/`; the suite fails on an unmatched step rather than
skipping it.

## UI changes require screenshot updates

After any change that alters the rendered UI (`static/css/`, `templates/`,
`static/js/`): `cargo build`, then `cd e2e && cargo run --bin screenshots`, and
include the result in the same change. The four images under `screenshots/` are
referenced by `README.md`, so stale screenshots count as part of the change. The
generator (`e2e/src/bin/screenshots.rs`) seeds demo data and captures the unread
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
  `secret.rs`. Session and credential lifecycle events (creation, renewal,
  token rotation, destruction, masquerade start/stop, re-authentication,
  passkey added/removed) are audited under the `rdrs::audit` tracing target,
  identifying sessions only by a salted `secret::audit_id` hash. A new
  credential path must emit one — that log is the only record of a credential
  being added.
