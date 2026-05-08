# SSR-first PR-1: Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the three pieces of pure server-side infrastructure (brotli compression, in-process LRU page-cache primitive, ETag/304 middleware) that the SSR-first redesign depends on, without changing any existing route or rendering pipeline.

**Architecture:** Three independent additions, each with its own integration or unit test, each individually committed. Consumers (per-page SSR migrations) wire these in subsequent PRs. Touching no existing handler keeps the PR risk low and trivially revertable.

**Tech Stack:** Rust + Axum 0.8 + Tower middleware (`Layer`/`Service`), `tower-http` 0.6 (existing dep — add `compression-br` feature), `moka` 0.12 sync cache (existing dep), `sha2` 0.11 (existing dep — for weak ETag hashing). `axum-test` 20 + `tokio::test` (existing dev-deps) for integration tests.

**Spec:** `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`

**Branch:** `feat/ssr-first-redesign` (already created and contains the spec commit `3cc034c`).

---

## Pre-flight

- [ ] **Verify branch and clean state.**

  Run: `git status && git branch --show-current`
  Expected: branch `feat/ssr-first-redesign`, working tree clean (or only contains in-progress plan file).

- [ ] **Source OpenSSL env.**

  Run: `source /tmp/rdrs-env.sh`
  This is required on the dev machine before every cargo / e2e command (see `~/.claude/projects/-home-nixos-Develop-claude-rdrs/memory/project_openssl_env.md`).

- [ ] **Baseline test suite green.**

  Run: `cargo nextest run`
  Expected: all tests pass. If any fail on `main`, stop and surface to user.

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify | `Cargo.toml` | Add `compression-br` feature to existing `tower-http` entry. |
| Modify | `src/lib.rs` | Chain `.br(true)` onto `CompressionLayer`; register new `ETagLayer` ahead of existing layers. |
| Create | `src/services/page_cache.rs` | Thin generic helper around `moka::sync::Cache` for per-user TTL'd page-cache entries. Exposes `new_page_cache::<K, V>(capacity, ttl)`. No global state, no AppState change. |
| Modify | `src/services/mod.rs` | `pub mod page_cache;` + re-export. |
| Create | `src/middleware/etag.rs` | `ETagLayer` + `ETagService` — Tower middleware that buffers 2xx `text/html` response bodies, computes weak SHA-256 ETag, and converts to 304 when `If-None-Match` matches. Modeled after existing `date_header.rs`. |
| Modify | `src/middleware/mod.rs` | `pub mod etag; pub use etag::ETagLayer;` |
| Modify | `tests/compression_test.rs` | Add `test_login_page_brotli_when_accepted` matching the existing gzip test pattern. |
| Create | `tests/etag_test.rs` | Integration tests: ETag header present on 200, 304 on matching `If-None-Match`, non-text responses untouched. |
| Create | `tests/page_cache_test.rs` | Unit-style integration test (in `tests/` for visibility): insert/get/invalidate/TTL expiry. |

Nothing under `static/`, `templates/`, or any `handlers/*.rs` is touched.

---

## Task 1: Brotli compression

Adds brotli alongside the existing gzip compression. Pure tower-http feature flag + one method call.

**Files:**
- Modify: `Cargo.toml` (line with `tower-http = { version = "0.6", features = [...] }`)
- Modify: `src/lib.rs` (line `.layer(CompressionLayer::new().gzip(true))` ≈ `src/lib.rs:198`)
- Test: `tests/compression_test.rs` (add new test, do not delete existing gzip tests)

- [ ] **Step 1: Write the failing test.**

  Append to `tests/compression_test.rs` (above the closing of the file, after `test_login_page_not_compressed_without_accept_encoding`):

  ```rust
  #[tokio::test]
  async fn test_login_page_brotli_when_accepted() {
      let server = create_test_server(default_test_config());

      let response = server
          .get("/login")
          .add_header(header::ACCEPT_ENCODING, HeaderValue::from_static("br"))
          .await;

      response.assert_status_ok();
      let encoding = response.headers().get(header::CONTENT_ENCODING).expect(
          "CompressionLayer should set Content-Encoding when client sends Accept-Encoding: br",
      );
      assert_eq!(encoding.to_str().unwrap(), "br");
  }
  ```

- [ ] **Step 2: Run the new test to verify it fails.**

  Run: `cargo nextest run --test compression_test test_login_page_brotli_when_accepted`
  Expected: test fails — either `Content-Encoding` header is missing, or its value is `gzip` / `identity`. (Without the `compression-br` feature and `.br(true)`, tower-http won't honor `Accept-Encoding: br`.)

- [ ] **Step 3: Add the `compression-br` feature.**

  Edit `Cargo.toml`. Locate:
  ```toml
  tower-http = { version = "0.6", features = ["compression-gzip", "timeout", "trace"] }
  ```
  Replace with:
  ```toml
  tower-http = { version = "0.6", features = ["compression-br", "compression-gzip", "timeout", "trace"] }
  ```

- [ ] **Step 4: Enable brotli on the CompressionLayer.**

  Edit `src/lib.rs`. Locate (around line 198):
  ```rust
          .layer(CompressionLayer::new().gzip(true))
  ```
  Replace with:
  ```rust
          .layer(CompressionLayer::new().gzip(true).br(true))
  ```

- [ ] **Step 5: Run the new test to verify it passes.**

  Run: `cargo nextest run --test compression_test test_login_page_brotli_when_accepted`
  Expected: PASS.

- [ ] **Step 6: Run the full compression test file to confirm no regression.**

  Run: `cargo nextest run --test compression_test`
  Expected: 3 tests pass (existing gzip-when-accepted, no-encoding-without-header, new brotli test).

- [ ] **Step 7: Run the full test suite.**

  Run: `cargo nextest run`
  Expected: all green.

- [ ] **Step 8: Format and commit.**

  Run: `cargo fmt`
  Then check `git status` — `Cargo.lock` will have been updated by `cargo build`/`cargo nextest` to record the new transitive `brotli` crate. Stage it together with the source changes:
  ```bash
  git add Cargo.toml Cargo.lock src/lib.rs tests/compression_test.rs
  git commit -m "$(cat <<'EOF'
  perf(http): enable brotli compression alongside gzip

  Adds the `compression-br` feature on tower-http and chains
  `.br(true)` onto the existing CompressionLayer. Saves ~15-25%
  bytes vs gzip on HTML/CSS/JS for clients that advertise
  Accept-Encoding: br.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 2: LRU page-cache primitive

Adds `services/page_cache.rs` exposing a tiny helper for building per-user TTL-bounded caches keyed by `(user_id, ...)`. No callers wired up in this PR — the helper sits available for per-page SSR PRs to pick up.

**Files:**
- Create: `src/services/page_cache.rs`
- Modify: `src/services/mod.rs`
- Test: `tests/page_cache_test.rs`

- [ ] **Step 1: Write the failing test.**

  Create `tests/page_cache_test.rs`:

  ```rust
  //! Verifies the page_cache helper: insert/get round-trip, manual
  //! invalidation, and TTL expiry. The helper is a thin wrapper over
  //! `moka::sync::Cache`; these tests pin the contract page-handlers
  //! will rely on.

  use std::time::Duration;

  use rdrs::services::page_cache::new_page_cache;

  #[test]
  fn insert_and_get_roundtrip() {
      let cache = new_page_cache::<(i64, &'static str), String>(64, Duration::from_secs(60));

      cache.insert((1, "sidebar"), "payload-A".to_string());

      assert_eq!(cache.get(&(1, "sidebar")), Some("payload-A".to_string()));
      assert_eq!(cache.get(&(1, "feeds")), None);
      assert_eq!(cache.get(&(2, "sidebar")), None);
  }

  #[test]
  fn invalidate_removes_entry() {
      let cache = new_page_cache::<(i64, &'static str), String>(64, Duration::from_secs(60));

      cache.insert((1, "sidebar"), "payload-A".to_string());
      cache.invalidate(&(1, "sidebar"));

      assert_eq!(cache.get(&(1, "sidebar")), None);
  }

  #[tokio::test]
  async fn ttl_expiry_drops_entry() {
      let cache = new_page_cache::<(i64, &'static str), String>(64, Duration::from_millis(50));

      cache.insert((1, "sidebar"), "payload-A".to_string());
      assert_eq!(cache.get(&(1, "sidebar")), Some("payload-A".to_string()));

      tokio::time::sleep(Duration::from_millis(120)).await;
      // moka requires a sync_or_pending operation to advance expiry;
      // a get() call is sufficient.
      assert_eq!(cache.get(&(1, "sidebar")), None);
  }
  ```

- [ ] **Step 2: Run the new test to verify it fails.**

  Run: `cargo nextest run --test page_cache_test`
  Expected: compile error — `rdrs::services::page_cache` does not exist.

- [ ] **Step 3: Create the helper module.**

  Create `src/services/page_cache.rs`:

  ```rust
  //! Thin helper around `moka::sync::Cache` for per-user, TTL-bounded
  //! page caches.
  //!
  //! Page handlers wire one `Cache` per logical kind of payload they
  //! want to memoize (sidebar tree, feeds list, statistics rollup).
  //! CRUD paths invalidate explicitly via `Cache::invalidate(&key)`.
  //!
  //! This module deliberately does not own any global state — the
  //! caches live in `AppState` (added when the first per-page PR
  //! needs one).

  use std::hash::Hash;
  use std::time::Duration;

  use moka::sync::Cache;

  /// Build a new page cache with the given capacity and per-entry
  /// time-to-live.
  ///
  /// `capacity` is the maximum number of entries (an LRU bound, not
  /// a byte bound). `ttl` is the time-to-live applied to each entry
  /// from insertion.
  pub fn new_page_cache<K, V>(capacity: u64, ttl: Duration) -> Cache<K, V>
  where
      K: Hash + Eq + Send + Sync + 'static,
      V: Clone + Send + Sync + 'static,
  {
      Cache::builder()
          .max_capacity(capacity)
          .time_to_live(ttl)
          .build()
  }
  ```

- [ ] **Step 4: Wire the module into `services/mod.rs`.**

  Edit `src/services/mod.rs`. Locate the block of `pub mod` declarations near the top and add `page_cache`:

  ```rust
  pub mod page_cache;
  ```

  Add it alphabetically (after `opml`, before `readability`).

- [ ] **Step 5: Run the new test to verify it passes.**

  Run: `cargo nextest run --test page_cache_test`
  Expected: 3 tests pass (`insert_and_get_roundtrip`, `invalidate_removes_entry`, `ttl_expiry_drops_entry`).

- [ ] **Step 6: Run the full test suite.**

  Run: `cargo nextest run`
  Expected: all green.

- [ ] **Step 7: Format and commit.**

  Run: `cargo fmt`
  Then:
  ```bash
  git add src/services/page_cache.rs src/services/mod.rs tests/page_cache_test.rs
  git commit -m "$(cat <<'EOF'
  feat(services): add page_cache helper over moka::sync::Cache

  Tiny generic helper for per-user, TTL-bounded page caches.
  Per-page SSR migrations will wire one cache per payload kind
  (sidebar, feeds list, statistics rollup) and invalidate on CRUD.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 3: ETag middleware

Adds a Tower middleware that:

1. On 2xx responses with `Content-Type: text/html...`, buffers the body, computes a weak ETag (`W/"<sha256-prefix-hex>"`), and sets the `ETag` header.
2. If the request carried `If-None-Match` matching the computed ETag, replaces the response with `304 Not Modified` (empty body, headers preserved minus `Content-Length` adjustments).

Non-HTML responses pass through untouched. The middleware is wired innermost so it sees the uncompressed body; `CompressionLayer` runs after it on the response path.

**Files:**
- Create: `src/middleware/etag.rs`
- Modify: `src/middleware/mod.rs`
- Modify: `src/lib.rs` (add `.layer(middleware::ETagLayer::new())` as the FIRST `.layer()` call after `.with_state(state)`)
- Test: `tests/etag_test.rs`

- [ ] **Step 1: Write the failing test.**

  Create `tests/etag_test.rs`:

  ```rust
  //! Verifies the ETagLayer:
  //! - 2xx HTML responses get a weak ETag header.
  //! - Repeating the request with If-None-Match returns 304.
  //! - Non-HTML responses are not touched.

  use std::sync::Arc;

  use axum::http::{header, HeaderValue, StatusCode};
  use axum_test::TestServer;
  use rdrs::{auth, create_router, db, services, AppState, Config, DbPool};
  use rusqlite::Connection;

  fn open_shared_memory(name: &str) -> Connection {
      let uri = format!("file:{}?mode=memory&cache=shared", name);
      Connection::open_with_flags(
          uri,
          rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
              | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
              | rusqlite::OpenFlags::SQLITE_OPEN_URI,
      )
      .unwrap()
  }

  fn default_test_config() -> Config {
      Config {
          database_url: ":memory:".to_string(),
          server_port: 3000,
          signup_enabled: true,
          multi_user_enabled: true,
          image_proxy_secret: vec![0u8; 32],
          image_proxy_secret_generated: false,
          user_agent: "RDRS-Test/1.0".to_string(),
          webauthn_rp_id: "localhost".to_string(),
          webauthn_rp_origin: "http://localhost:3000".to_string(),
          webauthn_rp_name: "rdrs-test".to_string(),
          public_base_url: None,
      }
  }

  fn create_test_server(name: &str, config: Config) -> TestServer {
      let write_conn = open_shared_memory(name);
      db::init_db(&write_conn).unwrap();
      let read_conn = open_shared_memory(name);

      let (db, _handle) = DbPool::new(write_conn, read_conn);
      let webauthn = auth::create_webauthn(&config).unwrap();
      let summary_cache = services::create_summary_cache(100, 24);
      let (summary_tx, _summary_rx) = services::create_summary_channel(10);

      let state = AppState {
          db,
          config: Arc::new(config),
          webauthn: Arc::new(webauthn),
          summary_cache,
          summary_tx,
      };

      TestServer::builder().build(create_router(state))
  }

  #[tokio::test]
  async fn html_response_carries_weak_etag() {
      let server = create_test_server("etag_test_a", default_test_config());

      let response = server.get("/login").await;

      response.assert_status_ok();
      let etag = response
          .headers()
          .get(header::ETAG)
          .expect("HTML response should carry ETag");
      let value = etag.to_str().unwrap();
      assert!(value.starts_with("W/\""), "expected weak ETag, got {value}");
      assert!(value.ends_with('"'), "expected closing quote, got {value}");
  }

  #[tokio::test]
  async fn matching_if_none_match_returns_304() {
      let server = create_test_server("etag_test_b", default_test_config());

      let first = server.get("/login").await;
      first.assert_status_ok();
      let etag = first
          .headers()
          .get(header::ETAG)
          .expect("first response should carry ETag")
          .clone();

      let second = server
          .get("/login")
          .add_header(header::IF_NONE_MATCH, etag.clone())
          .await;

      second.assert_status(StatusCode::NOT_MODIFIED);
      // 304 must echo the ETag header.
      assert_eq!(second.headers().get(header::ETAG), Some(&etag));
      // 304 has no body.
      assert!(second.as_bytes().is_empty());
  }

  #[tokio::test]
  async fn non_html_response_has_no_etag() {
      // /favicon.svg returns image/svg+xml — should not be tagged.
      let server = create_test_server("etag_test_c", default_test_config());

      let response = server.get("/favicon.svg").await;

      response.assert_status_ok();
      assert!(
          response.headers().get(header::ETAG).is_none(),
          "non-HTML responses must not be tagged"
      );
  }

  #[tokio::test]
  async fn non_matching_if_none_match_returns_full_body() {
      let server = create_test_server("etag_test_d", default_test_config());

      let response = server
          .get("/login")
          .add_header(
              header::IF_NONE_MATCH,
              HeaderValue::from_static("W/\"deadbeef\""),
          )
          .await;

      response.assert_status_ok();
      assert!(
          !response.as_bytes().is_empty(),
          "non-matching If-None-Match should return full body"
      );
  }
  ```

- [ ] **Step 2: Run the new tests to verify they fail.**

  Run: `cargo nextest run --test etag_test`
  Expected: tests run but fail at runtime — the binary still builds (the new test file does not import `ETagLayer` directly; it goes through `create_router`), but `html_response_carries_weak_etag` FAILS because no `ETag` header is set, and `matching_if_none_match_returns_304` FAILS because the response is 200 not 304.

- [ ] **Step 3: Create the middleware module.**

  Create `src/middleware/etag.rs`:

  ```rust
  //! Tower middleware that attaches a weak ETag to 2xx text/html
  //! responses and converts to 304 when the client's If-None-Match
  //! matches.
  //!
  //! Wired innermost so the body it hashes is the uncompressed one;
  //! CompressionLayer runs after this on the response path.

  use std::pin::Pin;
  use std::task::{Context, Poll};

  use axum::body::{to_bytes, Body};
  use axum::http::{header, HeaderValue, Request, StatusCode};
  use axum::response::Response;
  use sha2::{Digest, Sha256};
  use tower::{Layer, Service};

  /// Maximum body size (bytes) the middleware will buffer to compute
  /// an ETag. Bodies larger than this pass through untouched.
  /// 4 MiB covers any reasonable SSR HTML page; anything larger is
  /// almost certainly a streamed asset and shouldn't be buffered.
  const MAX_BUFFER_BYTES: usize = 4 * 1024 * 1024;

  #[derive(Clone, Default)]
  pub struct ETagLayer;

  impl ETagLayer {
      pub fn new() -> Self {
          Self
      }
  }

  impl<S> Layer<S> for ETagLayer {
      type Service = ETagService<S>;

      fn layer(&self, inner: S) -> Self::Service {
          ETagService { inner }
      }
  }

  #[derive(Clone)]
  pub struct ETagService<S> {
      inner: S,
  }

  impl<S> Service<Request<Body>> for ETagService<S>
  where
      S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
      S::Future: Send + 'static,
  {
      type Response = S::Response;
      type Error = S::Error;
      type Future = Pin<Box<dyn std::future::Future<Output = Result<Self::Response, S::Error>> + Send>>;

      fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
          self.inner.poll_ready(cx)
      }

      fn call(&mut self, request: Request<Body>) -> Self::Future {
          let if_none_match = request
              .headers()
              .get(header::IF_NONE_MATCH)
              .and_then(|v| v.to_str().ok())
              .map(|s| s.to_string());

          let mut inner = self.inner.clone();

          Box::pin(async move {
              let response = inner.call(request).await?;

              if !is_taggable(&response) {
                  return Ok(response);
              }

              let (mut parts, body) = response.into_parts();
              let bytes = match to_bytes(body, MAX_BUFFER_BYTES).await {
                  Ok(b) => b,
                  Err(_) => {
                      // Body too large or stream error — return a fresh
                      // empty 200 with a hint header. Production handlers
                      // should not exceed MAX_BUFFER_BYTES for HTML; if
                      // they do we want a loud signal in tests.
                      let response = Response::builder()
                          .status(StatusCode::INTERNAL_SERVER_ERROR)
                          .body(Body::empty())
                          .expect("static response");
                      return Ok(response);
                  }
              };

              let etag = compute_weak_etag(&bytes);
              parts.headers.insert(
                  header::ETAG,
                  HeaderValue::from_str(&etag).expect("etag is ascii"),
              );

              if let Some(client_value) = if_none_match {
                  if etag_matches(&client_value, &etag) {
                      parts.status = StatusCode::NOT_MODIFIED;
                      parts.headers.remove(header::CONTENT_LENGTH);
                      parts.headers.remove(header::CONTENT_TYPE);
                      return Ok(Response::from_parts(parts, Body::empty()));
                  }
              }

              Ok(Response::from_parts(parts, Body::from(bytes)))
          })
      }
  }

  fn is_taggable(response: &Response) -> bool {
      if !response.status().is_success() {
          return false;
      }
      response
          .headers()
          .get(header::CONTENT_TYPE)
          .and_then(|v| v.to_str().ok())
          .map(|ct| ct.starts_with("text/html"))
          .unwrap_or(false)
  }

  fn compute_weak_etag(body: &[u8]) -> String {
      let mut hasher = Sha256::new();
      hasher.update(body);
      let digest = hasher.finalize();
      // First 16 hex chars = 64 bits — collision-safe for our use.
      let hex: String = digest.iter().take(8).map(|b| format!("{:02x}", b)).collect();
      format!("W/\"{hex}\"")
  }

  fn etag_matches(client: &str, server: &str) -> bool {
      // Accept exact match. Per RFC 7232, If-None-Match comparison is
      // weak (W/-prefix is ignored), so we strip W/ from both sides.
      let normalize = |s: &str| -> String {
          s.trim()
              .strip_prefix("W/")
              .unwrap_or(s.trim())
              .to_string()
      };
      // Client may send a comma-separated list; check each.
      client
          .split(',')
          .any(|entry| normalize(entry) == normalize(server))
  }
  ```

- [ ] **Step 4: Re-export from `middleware/mod.rs`.**

  Edit `src/middleware/mod.rs`. Replace its contents with:

  ```rust
  pub mod auth;
  pub mod date_header;
  pub mod etag;
  pub mod flash;

  pub use auth::{AdminUser, AuthUser, PageAdminUser, PageAuthUser, SESSION_COOKIE_NAME};
  pub use date_header::DateHeaderLayer;
  pub use etag::ETagLayer;
  pub use flash::{Flash, FlashMessage, FlashRedirect, SetFlash, FLASH_COOKIE_NAME};
  ```

- [ ] **Step 5: Wire the layer into `src/lib.rs`.**

  Edit `src/lib.rs`. Locate the bottom of `create_router`:

  ```rust
          .with_state(state)
          .layer(middleware::DateHeaderLayer::new())
          .layer(CompressionLayer::new().gzip(true).br(true))
          .layer(TimeoutLayer::with_status_code(
              axum::http::StatusCode::REQUEST_TIMEOUT,
              SERVER_REQUEST_TIMEOUT,
          ))
  ```

  Insert `.layer(middleware::ETagLayer::new())` as the FIRST `.layer()` after `.with_state(state)` so it is innermost (closest to the handler) and sees the uncompressed body:

  ```rust
          .with_state(state)
          .layer(middleware::ETagLayer::new())
          .layer(middleware::DateHeaderLayer::new())
          .layer(CompressionLayer::new().gzip(true).br(true))
          .layer(TimeoutLayer::with_status_code(
              axum::http::StatusCode::REQUEST_TIMEOUT,
              SERVER_REQUEST_TIMEOUT,
          ))
  ```

  Tower wraps in registration order (each `.layer()` adds an outer wrapper), so this places ETag at the bottom of the stack — response leaves the handler, hits ETag first (raw body), then DateHeader, then CompressionLayer, then TimeoutLayer.

- [ ] **Step 6: Run the new tests to verify they pass.**

  Run: `cargo nextest run --test etag_test`
  Expected: 4 tests pass (`html_response_carries_weak_etag`, `matching_if_none_match_returns_304`, `non_html_response_has_no_etag`, `non_matching_if_none_match_returns_full_body`).

- [ ] **Step 7: Run the full test suite.**

  Run: `cargo nextest run`
  Expected: all green. The brotli + gzip + handler tests must continue to pass; ETag must not break compression (verifies the layer ordering).

- [ ] **Step 8: Format and commit.**

  Run: `cargo fmt`
  Then:
  ```bash
  git add src/middleware/etag.rs src/middleware/mod.rs src/lib.rs tests/etag_test.rs
  git commit -m "$(cat <<'EOF'
  perf(http): add ETag middleware with 304 Not Modified support

  Buffers 2xx text/html responses, attaches a weak SHA-256 ETag, and
  short-circuits to 304 when the client's If-None-Match matches.
  Non-HTML responses pass through untouched. Wired innermost so the
  hash is computed on the uncompressed body; CompressionLayer
  follows.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Wrap-up

- [ ] **Final sweep.**

  Run: `cargo nextest run && cargo fmt --check`
  Expected: tests green, formatting clean.

- [ ] **Push branch.**

  Run: `git push -u origin feat/ssr-first-redesign`

- [ ] **Open the PR.**

  Run:
  ```bash
  gh pr create --title "feat(ssr): foundation — brotli, page_cache, ETag" --body "$(cat <<'EOF'
  ## Summary
  - Enable brotli alongside gzip (`tower-http` `compression-br` feature, `.br(true)`).
  - Add `services::page_cache::new_page_cache` — generic moka helper for per-user TTL-bounded page caches; no consumers yet.
  - Add `middleware::ETagLayer` — weak SHA-256 ETag on 2xx HTML, 304 on matching `If-None-Match`. Wired innermost so it sees uncompressed bodies.

  Pure additive infrastructure for the SSR-first redesign. No existing route is modified. Spec: `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`.

  ## Test plan
  - [x] `cargo nextest run` — full suite green
  - [x] New test: `tests/compression_test.rs::test_login_page_brotli_when_accepted`
  - [x] New tests: `tests/page_cache_test.rs` (insert/get, invalidate, TTL)
  - [x] New tests: `tests/etag_test.rs` (ETag header, 304 match, non-HTML untouched, non-match falls through)

  🤖 Generated with [Claude Code](https://claude.com/claude-code)
  EOF
  )"
  ```

---

## Subsequent PRs

Per the spec migration table, PRs 2-12 will receive their own plans authored when each is ready to start:

| # | Title (working) |
|---|----------------|
| 2 | Shell teardown — switch render pipeline to `base.html`, remove SPA router, add `swap()` helper, partials. |
| 3 | `/settings` SSR. |
| 4 | `/user-settings` SSR (passkey JS retained as exception). |
| 5 | `/admin` SSR. |
| 6 | `/statistics` SSR. |
| 7 | `/categories` SSR. |
| 8 | `/feeds` SSR. |
| 9 | `/search` SSR. |
| 10 | Entries family — `/`, `/entries`, `/entries/{read,starred,summarized}` SSR + reading-pane swap. |
| 11 | Entries family — `/feeds/{id}/entries`, `/categories/{id}/entries` SSR. |
| 12 | Cleanup — delete CSR shell + page modules + unused `/api/*` + e2e prune. |

Each plan will live at `docs/superpowers/plans/2026-MM-DD-ssr-first-prN-<topic>.md`.
