# CLAUDE.md

Guidance for Claude Code (claude.ai/code) in this repository.

## Commands

### Rust (repo root)

- Format (CI gate): `cargo fmt --all -- --check`; run `cargo fmt` before committing.
- Lint (CI gate): `cargo clippy --all-targets -- -D warnings`.
- Supply-chain (CI gate): `cargo deny check`.
- Build: `cargo build` / `cargo build --release`.
- Test: `cargo nextest run` (never `cargo test`); one test: `cargo nextest run <substring>`.
- Coverage (as CI): `RDRS_FAST_HASH=1 cargo llvm-cov nextest --lcov --output-path lcov.info`.
- Entry-list benchmark: `RDRS_FAST_HASH=1 cargo bench --bench entry_pages -- --label before`
  (`--entries`, `--iterations`). Server-side p50/p90 per list route; the
  baseline for list-handler or entry-query changes (`dom-bench` covers the browser).

`RDRS_FAST_HASH=1` uses minimal-cost Argon2 for fast local tests. **Never** set
it in production.

### E2E (cucumber + thirtyfour, from `e2e/`)

`e2e/` is a **separate cargo workspace** (keeps browser deps out of root
coverage), so root `cargo fmt --all` / `cargo clippy` don't reach it — lint it
separately.

- Run all: `cargo test --lib --test e2e` (`--lib` runs the wait-helper unit
  tests). One feature: `RDRS_E2E_FEATURES=features/reading.feature cargo test --test e2e`.
- Format / lint (CI gates): `cargo fmt --all -- --check`,
  `cargo clippy --all-targets -- -D warnings`.
- README screenshots: `cargo run --bin screenshots` (writes `../screenshots/`).
- CSP audit (CI gate): `cargo run --bin csp-audit` — fails on any browser CSP
  violation. Run after touching `templates/`, `static/css/` or `static/js/`
  (the source scan in `src/middleware/security_headers.rs` can't see runtime
  markup or shadow DOM).
- No-JS walkthrough (CI gate): `cargo run --bin nojs` — scripts disabled and
  every `*.js` request aborted.
- Touch-target report (not a gate): `cargo run --bin touch-audit`.
- Swap benchmark: `cargo run --bin dom-bench -- --label before` (`--entries`,
  `--rows`, `--mode`, `--profile`; see module docs). This is the before/after
  baseline for performance work.

**A browser must be installed** (`WebDriver::managed` fetches only the driver).
macOS: `brew install --cask ungoogled-chromium`; CI's `ubuntu-latest` has Chrome.

**Rebuild before E2E/screenshots.** Assets and templates are embedded at compile
time and the suite skips building if a binary exists. After editing `static/`,
`templates/`, or Rust source, run `cargo build` first.

**`.feature` files are the contract**, read as-is. New scenarios need steps in
`e2e/tests/e2e/steps/`; unmatched steps fail the suite.

## UI changes require screenshot updates

Any change to rendered UI (`static/css/`, `templates/`, `static/js/`):
`cargo build`, then `cd e2e && cargo run --bin screenshots`, and include the
four updated images under `screenshots/` (referenced by `README.md`) in the same
change. The generator (`e2e/src/bin/screenshots.rs`) captures the unread list
with reading pane and the keyboard-help overlay, light and dark.

## Architecture

**Askama templates → Axum handlers → services → models → SQLite/PostgreSQL.**
`ARCHITECTURE.md` is the full map. Rules most easily violated:

- **SSR-first, no frontend build tooling.** Logged-in pages are server-rendered;
  mutations are form POSTs answered with flash + redirect (`FlashRedirect`).
  `static/js/` is progressive enhancement only — vanilla ES modules via
  `include_str!`; no bundlers or transpilers.
- **Everything compiles into the single binary** (templates, CSS, JS,
  favicons) — hence the mandatory rebuild.
- **Every query uses the `query_*!` / `db_execute!` macros** (SQL and binds
  written once for both backends). Dialect forks go in `entry::filters::Dialect`
  or the `pg_rewrite` shim — except the entry upsert's NULL-safe inequality
  (`IS NOT` / `IS DISTINCT FROM`), a deliberate hand-dispatched `*_SQLITE` /
  `*_PG` pair, since an `IS NOT` rewrite would corrupt every `IS NOT NULL`.
  Migrations: `migrations/{sqlite,postgres}/`.
- **Background DB work must call `db.background()`** to yield to interactive
  SQLite writes (no-op on PostgreSQL).
- **`RDRS_SECRET` is the one root key**: session cookie, image proxy, GReader
  post token and CSRF token derive from it with domain separation in
  `secret.rs`. Session/credential lifecycle events (creation, renewal, token
  rotation, destruction, masquerade start/stop, re-authentication, passkey
  added/removed) are audited under the `rdrs::audit` tracing target, with
  sessions identified only by `secret::audit_id`. **A new credential path must
  emit an audit event.**
