# Slim Down rdrs — Design

Date: 2026-06-09
Branch: `refactor/slim-down`

## Goal

Reduce dependency surface, Docker build context, and source-code duplication
without changing any user-facing behavior. Three independent dimensions, chosen
by the user: dependency slimming, binary/Docker hygiene, and source DRY +
splitting oversized files.

Out of scope: local disk cleanup (`target/`, `*.sqlite3`), `opt-level` changes
(deferred — needs a perf benchmark gate per project rules), musl/static binary
port (high effort, real porting risk with ring/openssl-vendored).

## Findings (analysis-only, already done)

- No compiler dead-code warnings, no `#[allow(dead_code)]`, no orphan
  templates/JS, no commented-out code. The source is clean; the slack is
  **duplication**, not dead code.
- Release binary is already 15 MB with an optimal `[profile.release]`
  (lto, codegen-units=1, strip, panic=abort). Top crates are all load-bearing.

## Work items

### A. Dependencies (`Cargo.toml`)

1. **KEEP `openssl = { vendored }` — do NOT remove.** Verification (`cargo tree
   -e features -i openssl`) shows the `vendored` feature is enabled **only** by
   this direct dep; `webauthn-rs-core` pulls openssl with *default* features
   (system openssl). Dropping the direct dep would switch the build from a
   self-contained vendored build to requiring system `libssl-dev`/`pkg-config` —
   breaking both this NixOS box and the Dockerfile. The dep exists precisely to
   force the vendored build. (Original analysis was wrong; corrected at review.)
2. **Remove `rand_core` direct dep.** Only used via `password_hash::rand_core::OsRng`
   (`src/auth/password.rs:2`); the direct crate is referenced 0×.
3. **Drop `tower-http` `trace` feature.** No `TraceLayer` anywhere; only
   `CompressionLayer` + `TimeoutLayer` are used.
4. **Trim `tokio` `features=["full"]`** to
   `["macros","rt-multi-thread","time","net","signal","sync"]`. Verified usage:
   `time`, `spawn`/`task`/`select` (rt+macros), `sync::mpsc`, `signal::unix`,
   `net::TcpListener`; no `fs`/`process`/`io-util`/`io-std`. `#[tokio::test]` is
   covered by `macros`+`rt`.

Verification: `cargo build` + `cargo nextest run` both green; `cargo fmt`.

### B. Docker / build context

5. **`.dockerignore`:** add `e2e/` and `screenshots/` so `COPY . .` in the
   builder stage stops pulling **`e2e/node_modules` (65 MB)** and image assets
   into the build context. (`docs/` is already largely covered by `*.md`;
   `coverage/` does not exist — skip both.)

(Toolchain 1.95 vs Dockerfile base 1.96 is a build-speed nit, not size — note
only, no change unless asked.)

### C. Source DRY

6. **`src/models/entry.rs`:** extract the entry/feed/category `SELECT` column
   list (repeated 6×) into a `const`, reused at all sites.
7. **Tests:** factor the repeated register/login setup in `tests/handlers_test.rs`
   and `tests/pages_test.rs` into a shared helper (`tests/common/`), removing
   ~300–500 lines of boilerplate. Behavior of each test must be identical.

### D. Split oversized files

8. **`src/handlers/pages.rs` (~2,948 LOC):** extract pure helpers
   (JSON-for-script escaping, sidebar serialization, flash bootstrap, relative
   time / freshness formatting) into focused sibling modules. Handlers stay put.
9. **`src/models/entry.rs` (~2,337 LOC):** extract filter/continuation SQL
   builders into a sibling module (e.g. `entry_filters.rs`).

Splitting is mechanical (move + re-export / `mod`), no logic change.

## Constraints

- New branch (`refactor/slim-down`); GPG-signed commits; explicit `git add` by
  name (no `-A`/`.`).
- Each work item is an independent, behavior-preserving change. Logical commits
  per concern (A/B/C/D) on one branch, single PR (squash-merge on completion).
- Every change verified with `cargo build` + `cargo nextest run` + `cargo fmt`.

## Risks

- Tokio feature trim (B) is the only medium-risk item — a missed feature shows up
  as a compile error, caught immediately by the build. Low blast radius.
- File splits (D) risk accidental visibility/import breakage — caught by build.
