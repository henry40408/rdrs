# Architecture

This document describes the architecture of RDRS, a self-hosted RSS reader built with Rust.

## Overview

RDRS follows a layered architecture with clear separation of concerns:

```
┌─────────────────────────────────────────────────────┐
│              Templates (Askama HTML)                │
├─────────────────────────────────────────────────────┤
│           HTTP Layer (Axum Handlers)                │
├─────────────────────────────────────────────────────┤
│            Services (Business Logic)                │
├─────────────────────────────────────────────────────┤
│              Models (Data Access)                   │
├─────────────────────────────────────────────────────┤
│              Database (SQLite)                      │
└─────────────────────────────────────────────────────┘
```

## Directory Structure

```
src/
├── main.rs              # Entry point
├── lib.rs               # Router and app configuration
├── config.rs            # Environment configuration
├── error.rs             # Error types and HTTP responses
├── version.rs           # Build version information
│
├── db/
│   └── pool.rs          # Dual-backend (SQLite/PostgreSQL) sqlx pool + SQLite write-priority scheduler
│
├── models/              # Data models and database operations
│   ├── user.rs          # User accounts
│   ├── session.rs       # Session management
│   ├── feed.rs          # RSS feeds
│   ├── entry/           # Feed entries (mod.rs + filters.rs query builder + query.rs boolean search parser)
│   ├── entry_summary.rs # Article summaries
│   ├── category.rs      # Feed categories
│   ├── image.rs         # Image storage
│   ├── statistics.rs    # Statistics/analytics queries
│   ├── passkey.rs       # WebAuthn credentials
│   ├── webauthn_challenge.rs # WebAuthn challenge state
│   └── user_settings.rs # User preferences
│
├── handlers/            # HTTP request handlers
│   ├── pages/           # HTML page rendering (mod.rs + script_json/search_text/time_format helpers)
│   ├── auth.rs          # Authentication endpoints
│   ├── passkey.rs       # Passkey/WebAuthn endpoints
│   ├── admin.rs         # Admin operations
│   ├── user.rs          # User operations + sidebar payload
│   ├── categories.rs    # Category form actions (SSR)
│   ├── feeds.rs         # Feed form actions: create/edit/delete/refresh/OPML import (SSR)
│   ├── feed.rs          # Per-feed JSON endpoints (e.g. icon)
│   ├── entries.rs       # Entry SSR fragments + form actions (read/star/summarize/save)
│   ├── entry.rs         # Per-entry JSON endpoints (summary, neighbors, full content)
│   ├── favicon.rs       # Favicon serving (embedded at compile time)
│   ├── static_assets.rs # Static JS assets (embedded at compile time)
│   ├── proxy.rs         # Image proxy
│   ├── health.rs        # Health check endpoint
│   ├── events.rs        # SSE live-update stream
│   └── greader/         # Google Reader API compatibility
│       ├── auth.rs      # ClientLogin authentication
│       ├── subscription.rs # Subscription list/edit, OPML import
│       ├── item.rs      # Stream contents and item IDs
│       ├── tag.rs       # Read/star tag operations
│       ├── user.rs      # User info endpoint
│       └── types.rs     # Shared GReader types
│
├── utils/               # Shared utility modules
│   ├── datetime.rs      # Date/time parsing (RFC 2822, ISO 8601, Chinese dates)
│   └── url_validation.rs# URL validation and SSRF protection
│
├── services/            # Business logic
│   ├── background.rs    # Background sync scheduler
│   ├── entry_retention.rs # Read-entry retention/pruning worker
│   ├── events.rs        # In-memory EventBus for SSE live updates
│   ├── feed_sync.rs     # Feed refresh logic
│   ├── feed_discovery.rs# Feed URL detection
│   ├── readability.rs   # Content extraction
│   ├── sanitize.rs      # HTML sanitization
│   ├── html_entities.rs # HTML entity decoding for plain-text fields
│   ├── opml.rs          # OPML import/export
│   ├── icon_fetcher.rs  # Feed icon fetching
│   ├── http.rs          # Shared HTTP client utilities
│   ├── image_proxy.rs   # Secure image proxying
│   ├── page_cache.rs    # Per-user TTL page-payload caches (moka)
│   ├── sidebar_cache.rs # Per-user sidebar chrome cache
│   ├── summary_cache.rs # Summary caching
│   ├── summary_cleanup.rs # Summary cleanup task
│   ├── summary_worker.rs# Summary generation worker
│   ├── save/            # External save targets
│   │   ├── mod.rs       # Save dispatch
│   │   └── linkding.rs  # Linkding integration
│   └── summarize/       # AI summarization
│       ├── mod.rs       # Summarizer trait
│       └── kagi.rs      # Kagi AI service
│
├── middleware/          # HTTP middleware
│   ├── auth.rs          # Session authentication
│   ├── date_header.rs   # Date response header
│   ├── etag.rs          # ETag / conditional-request handling
│   ├── flash.rs         # Flash messages
│   └── forward_auth.rs  # Forward-auth / trusted-header browser login
│
└── auth/
    ├── password.rs      # Password hashing (Argon2)
    └── webauthn.rs      # WebAuthn/Passkey authentication

templates/               # Askama HTML templates
tests/                   # Integration tests
```

## Core Components

### Application Entry (`main.rs`)

Initializes configuration, database connection, background tasks, and starts the Axum server.

### Router (`lib.rs`)

Defines all HTTP routes and builds the Axum application with:
- Embedded static asset serving (compiled into binary via `include_str!`/`include_bytes!`)
- Cookie layer for sessions
- Database connection pool as state

### Configuration (`config.rs`)

Loads settings from environment variables:
- `DATABASE_URL` - Database backend: a file path or `sqlite://` URL (SQLite, the zero-config default) or a `postgres://` URL (PostgreSQL); chosen once at startup
- `SERVER_PORT` - HTTP port
- `RDRS_SIGNUP_ENABLED` / `RDRS_MULTI_USER_ENABLED` - Registration settings
- `RDRS_SECRET` - Root HMAC key for every signature rdrs produces (session cookies, image-proxy URLs, GReader post token), each domain-separated in `secret.rs`
- `RDRS_AUTH_PROXY_HEADER` - Header name carrying the username from a forward-auth proxy; empty disables the feature
- `RDRS_TRUSTED_PROXY_NETWORKS` - Comma-separated CIDRs/IPs whose TCP peer is allowed to supply the identity header; required when `RDRS_AUTH_PROXY_HEADER` is set
- `RDRS_AUTH_PROXY_USER_CREATION` - Whether to JIT-create an account for an unknown proxy-provided username (`false` by default; on mismatch, redirects to `/login`)
- `RDRS_AUTH_PROXY_GROUPS_HEADER` - Header name carrying comma-separated group names from the proxy
- `RDRS_AUTH_PROXY_ADMIN_GROUP` - Group membership grants the admin role; active only when both this and `RDRS_AUTH_PROXY_GROUPS_HEADER` are set
- `RDRS_DISABLE_LOCAL_AUTH` - Hides the browser password form and makes `POST /api/session` return 403; does not affect GReader `ClientLogin` or passkey auth; startup refuses if set without `RDRS_AUTH_PROXY_HEADER`
- `RDRS_AUTH_PROXY_LOGOUT_URL` - When set, Sign Out redirects the browser here to end the IdP/SSO session (e.g. Authelia's logout URL); when unset, a forward-auth Sign Out clears the local session but warns the user to log out at the proxy instead of redirecting, since the proxy header would just re-authenticate them

### Error Handling (`error.rs`)

Custom `AppError` type that maps to appropriate HTTP responses:
- Authentication errors → 401
- Not found → 404
- Validation errors → 400
- Internal errors → 500

## Data Layer

### Schema & Migrations (`migrations/`)

Migrations are embedded per backend under `migrations/sqlite/` and `migrations/postgres/` and run at startup via `sqlx::migrate!` for the active backend. The two dialects are kept schema-equivalent; the genuine differences (e.g. non-id `INTEGER` columns are `BIGINT` on PostgreSQL, `GENERATED ALWAYS AS IDENTITY` ids) are isolated in the Postgres migration.

The schema has 11 tables:

| Table | Purpose |
|-------|---------|
| `user` | User accounts with role (admin/user) |
| `session` | Session tokens with masquerade support, the `previous_token`/`previous_token_expires_at` rotation grace pair, `last_authenticated_at` for the re-authentication window, plus per-session `user_agent`/`ip_address`/`last_seen_at` metadata |
| `category` | Feed categories per user |
| `feed` | Feed metadata with etag caching and bucket assignment |
| `entry` | Feed items with read/starred status |
| `entry_summary` | AI-generated article summaries |
| `entry_tombstone` | Tombstones for retention-pruned entries; prevents re-insertion on the next sync (cascades when the feed is deleted) |
| `image` | Polymorphic image storage |
| `user_settings` | User preferences and service configs |
| `passkey` | WebAuthn credential storage |
| `webauthn_challenge` | WebAuthn challenge state |

### Connection Pool (`db/pool.rs`)

`struct Db` wraps `enum DbInner { Sqlite(SqlitePool), Postgres(PgPool) }` — a single sqlx pool for whichever backend `DATABASE_URL` selected at startup. Every query flows through the `query_*!` / `db_execute!` dispatch macros, so SQL and binds are written once; the few genuine dialect differences are isolated behind `entry::filters::Dialect` and the `pg_rewrite` shim (`datetime('now')`→`now()`, `to_char` cursor comparisons, `make_interval`, quoted `"user"`). One fork suits neither: the entry upsert's NULL-safe inequality (`IS NOT` on SQLite, `IS DISTINCT FROM` on PostgreSQL) is a pair of `UPSERT_UPDATE_SQL_SQLITE` / `_PG` literals dispatched by hand in `models/entry/mod.rs`, because `pg_rewrite` substitutes blindly and a rule for `IS NOT` would also rewrite every `IS NOT NULL`. PG connections pin `TimeZone=UTC` so timestamp-string cursors stay byte-identical to SQLite.

**Write-priority scheduling (SQLite only).** SQLite has a single writer under WAL, so background writes must yield to interactive ones. `Db` carries a `Priority` (`User` by default in `AppState`; background workers call `db.background()`) and a shared `SqliteSched`: `admit()` gates a background write until no `User` write is in flight. Reads are never gated (WAL readers don't block the writer). On PostgreSQL this is a no-op — MVCC has real writer concurrency.

### Models

Each model provides:
- Struct definition matching database schema
- CRUD operations as associated functions (using params structs like `CreateFeedParams` to avoid excessive positional arguments)
- Query methods for common access patterns

Example: `Feed` model provides `find_by_user`, `create`, `update`, `delete`, `find_due_for_sync`.

The global `/search` page accepts a boolean query language (`is:`, `feed:`,
`category:`, `title:`, `author:`, `before:`, `after:`, `AND`/`OR`/`NOT`,
parenthesized grouping, quoting, and `-` negation), parsed by
`models/entry/query.rs` into a `QueryNode` AST and set on `EntryFilter.query`;
`filters::render_query` renders that AST to SQL per-`Dialect`, still matching
via `LIKE`/`ILIKE` (no full-text search index). Scoped, per-view search (feed,
category, starred, etc.) is unaffected and continues to use the plain
substring `EntryFilter.search` field.

## HTTP Layer

### Handlers

Request handlers are organized by resource:

- **pages.rs** - Renders HTML templates for browser navigation
- **auth.rs** - Login, register, logout
- **feed.rs** - Feed management, refresh, icon serving
- **entry.rs** - Entry reading, marking, searching
- **admin.rs** - User management for admins

### Middleware

- **auth.rs** - Extracts `AuthUser` from session cookie, provides `AdminUser` for admin-only routes
- **flash.rs** - Stores flash messages in cookies for UI feedback

### Authentication Flow

1. User submits credentials to `POST /api/session`
2. Server validates password with Argon2
3. Creates session record in database
4. Sets the signed session cookie (`<token>.<hmac>`, see [Signing & the root key](#signing--the-root-key-secretrs))
5. Subsequent requests extract user from `AuthUser` extractor, which verifies the signature before the DB lookup

### WebAuthn/Passkey Authentication

RDRS supports passwordless authentication via WebAuthn/Passkey:

**Registration Flow:**
1. User initiates passkey registration from settings
2. Server generates challenge and stores in `webauthn_challenge` table
3. Browser prompts user to create passkey (biometric/security key)
4. Client sends attestation to server
5. Server validates and stores credential in `passkey` table

**Authentication Flow:**
1. User clicks "Login with Passkey"
2. Server generates authentication challenge
3. Browser prompts user to verify passkey
4. Client sends assertion to server
5. Server validates signature and creates session

### Forward-Auth (Trusted-Header) Login

RDRS supports delegating browser authentication to an external forward-auth proxy (e.g., Authelia, authentik, Traefik ForwardAuth). When enabled, a Tower middleware (`middleware/forward_auth.rs`) intercepts browser page requests and attempts to establish a session from a trusted identity header before falling back to the normal cookie login flow.

> **Operator setup** — the environment variables, reverse-proxy requirements (header stripping, GReader and image-proxy path bypass), and logout behavior live in [README.md → Authentication & SSO](README.md#authentication--sso). This section documents the internal mechanics.

**Trust model:**

The middleware checks the TCP peer IP of the incoming connection against a set of trusted CIDRs/IPs (`RDRS_TRUSTED_PROXY_NETWORKS`). The peer address comes from the connection itself (`ConnectInfo`), not from `X-Forwarded-For`, so it cannot be spoofed by a downstream client. If the peer is untrusted, the identity header is ignored and the request proceeds to the normal session-cookie check. The middleware fails closed: any of untrusted peer, missing header, absent `ConnectInfo`, or DB error leaves the user unauthenticated.

**Username mapping (no schema change):**

The proxy-provided username is matched against existing rdrs accounts by username. No database migration is required. Existing password accounts continue to work and automatically gain forward-auth login when their username matches the proxy-provided value.

**JIT account creation:**

When `RDRS_AUTH_PROXY_USER_CREATION=true`, a proxy-provided username that matches no existing account causes a new local account to be created with a sentinel password hash (`"!"`) that cannot match any real password input, making local password login impossible for that account.

**Group → role sync:**

When both `RDRS_AUTH_PROXY_GROUPS_HEADER` and `RDRS_AUTH_PROXY_ADMIN_GROUP` are set, the user's role is recomputed from the groups header on every forward-auth login and persisted if it changed. The proxy/IdP is authoritative for role assignment while this mapping is active.

**`RDRS_DISABLE_LOCAL_AUTH` scope:**

Setting `RDRS_DISABLE_LOCAL_AUTH=true` hides the browser password-entry form and makes `POST /api/session` return HTTP 403. It does **not** affect GReader `ClientLogin` (`/accounts/ClientLogin`) or WebAuthn/passkey authentication, so native RSS clients and passkey users are unaffected.

**Middleware scope:**

The middleware is applied only to browser page routes. It is never invoked for the prefixes `/api`, `/reader`, `/accounts`, `/events`, `/static`, `/favicon`, and `/health`. It also skips requests that already carry a valid session cookie, so it adds no overhead for already-logged-in users.

**Logout mechanics:**

Sign Out always clears the local `session_token` cookie (with `Path=/`) and deletes the server-side session. The forward-auth middleware re-authenticates whenever there is no *valid* session cookie — a stale or expired cookie no longer blocks re-authentication, which is what prevents a logout lockout under forward-auth. `/login` redirects an already-authenticated user to `/` rather than rendering the login form again.

Because of that re-authentication, a local-only logout cannot actually end a forward-auth session when no `RDRS_AUTH_PROXY_LOGOUT_URL` is configured: the client would just get bounced back into the app on the very next request. `DELETE /api/session` reports `logout_url_configured` so the frontend (`rdrs-sidebar.js`) can decide whether to navigate to `redirect_to` (an absolute IdP URL or a same-host path), alongside `via_forward_auth` and `redirect_to` for the client to handle the logout flow correctly — it redirects to `redirect_to` and shows "You have been logged out." for a normal session, but for a forward-auth session with no configured logout URL it does not navigate at all and instead warns the user to log out at their proxy or SSO provider. The user-facing logout behavior and the `RDRS_AUTH_PROXY_LOGOUT_URL` knob are described in the README.

**Active session list:**

`/user-settings` lists the current user's non-expired sessions, showing device (`user_agent`), `ip_address`, created/last-active/expires times — no token or id reaches the template. `POST /user-settings/sessions/revoke-others` deletes every one of the user's sessions except the one making the request (`session::delete_user_sessions_except`), letting a user sign out other devices/browsers without ending their own session.

Every `session` row is now created with mandatory `user_agent`, `ip_address`, and `last_seen_at` columns (all `NOT NULL`), captured at login time from the request that authenticated (the 4 sites: `POST /api/session`, forward-auth auto-login, passkey `finish_authentication`, and GReader `ClientLogin`). The client IP is resolved by `Config::client_ip`, which only honours `X-Forwarded-For`/`X-Real-IP` when the TCP peer is a trusted proxy per `RDRS_TRUSTED_PROXY_NETWORKS` (the same `is_trusted_peer` check used by forward-auth); when trusted, it reads `X-Forwarded-For` right-to-left and takes the right-most entry that is not itself a trusted proxy (append-mode proxies like nginx's `$proxy_add_x_forwarded_for`, Traefik, and Caddy add each hop's observed address on the right, so a left-most read would let the client's own spoofed prefix win), falling back to `X-Real-IP` and then the TCP peer. Because untrusted entries closer to the client are never believed and an untrusted peer's headers are ignored outright, a client cannot spoof its logged IP. `last_seen_at` is bumped by the `AuthUser`/`PageAuthUser` extractors on every authenticated request, throttled to at most once per minute per session (`session::touch_last_seen`) to avoid a write on every request.

Because these columns are `NOT NULL` with no default, migration `0002_add_session_metadata` **drops and recreates the `session` table**, deleting all existing sessions — this is a breaking upgrade: every user is signed out and must log in again after upgrading.

**Auth-mode indicator:**

The sidebar shows an **SSO** pill when the current request is served through forward-auth — computed per request from the trusted proxy header (no stored state), surfaced via `via_forward_auth` on the auth extractors and the sidebar payload. The App page (`/settings`) lists the forward-auth configuration under a grouped Configuration table. That page is **admin-only** (`PageAdminUser`) because it exposes deployment internals, and the `DATABASE_URL` it shows is passed through `config::redact_database_url` so a PostgreSQL password never reaches the response; the sidebar hides its link for non-admins.

## Services

### Feed Synchronization

**Background Scheduler** (`background.rs`):
- Runs continuously in a Tokio task
- Distributes feeds across 60-minute buckets based on URL hash (stored as `bucket` column for indexed lookup)
- Syncs feeds in the current bucket every minute

**Sync Logic** (`feed_sync.rs`):
- Uses etag/if-modified-since for efficient updates
- Parses feed with feed-rs library (with custom timestamp parser for Chinese date support)
- Inserts new entries, skips duplicates
- Processes feeds in parallel using `tokio::task::JoinSet` with a concurrency limit of 4

### Entry Retention

**Retention Worker** (`entry_retention.rs`):
- Opt-in per user via `user_settings.retention_read_days` (`0` = disabled); a no-op when nobody has opted in.
- Runs every 24 hours, pruning entries that are read, older than the configured window, and not starred, in batches.
- Each pruned entry records an `entry_tombstone` (`feed_id`, `guid`) so the next feed sync does not re-insert it. Tombstones cascade-delete with their feed.

### Content Processing

**HTML Sanitization** (`sanitize.rs`):
- Uses Ammonia for XSS protection
- Removes tracking parameters (utm_*, fbclid, etc.)
- Blocks tracking domains (pixel.*, analytics.*, etc.)
- Removes 1x1 tracking pixels
- Fixes relative image URLs
- For images lacking `width`/`height`, harvests intrinsic dimensions (from `data-original-width`/`-height` or inline `style`) and injects them so the browser can reserve space
- Tags proxied content images with `data-img-state="loading"`; the reading pane shows a CSS skeleton until the image loads and swaps in a broken-image fallback (with `alt`) on error

**Full Content Extraction** (`readability.rs`):
- Fetches article URL
- Extracts main content using readability algorithm
- SSRF protection via shared `utils/url_validation` module (blocks private IPs, localhost, internal domains)

### Image Proxy (`image_proxy.rs`)

Proxies external images through the server to:
- Protect user privacy (no direct requests to external servers from clients)
- Work around mixed content issues (HTTPS pages with HTTP images)

Uses HMAC-SHA256 signatures to prevent abuse:
1. Server generates signed URL: `/api/proxy/image?url=...&s=...`
2. Proxy handler verifies signature before fetching the image
3. Signature is truncated to 8 bytes for URL brevity

**Caching:** responses carry `Cache-Control: public, max-age=86400` and an
`ETag` equal to the per-URL signature. A conditional request with a matching
`If-None-Match` is answered `304 Not Modified` without re-fetching the origin
(the image is immutable per URL), keeping refreshes / post-TTL revisits cheap
instead of re-downloading every image.

**URL Format:**
- **Relative paths** (default): `/api/proxy/image?url=...&s=...`
  - Used by Web UI (browsers automatically resolve relative paths)
  - Backward compatible behavior when `RDRS_PUBLIC_BASE_URL` is not configured
- **Absolute URLs** (optional): `https://rdrs.example.com/api/proxy/image?url=...&s=...`
  - Used by Google Reader API when `RDRS_PUBLIC_BASE_URL` is configured
  - Required for native RSS clients (e.g., NetNewsWire) that render HTML directly
  - Configured via `RDRS_PUBLIC_BASE_URL` environment variable

### SSE Live Updates

A single `GET /events` endpoint (`handlers/events.rs`) streams per-user Server-Sent Events to each open browser tab. Mutation paths (mark-read, mark-unread, mark-all, summarize, etc.) call `EventBus::emit_sidebar` or `emit_summary` on the shared in-memory `EventBus` (`services/events.rs`), which is a thin wrapper over a `tokio::sync::broadcast` channel. The browser's `EventSource` (wired up in `installSse()` in `static/js/app.js`) handles two event types:

- **`sidebar`** — triggers a notify-and-fetch refresh of `/api/sidebar`, updating the unread/summarized badge counts without a page reload.
- **`summary`** — carries `{entry_id, status}` JSON; the client rewrites the entry-row badge and, if that entry is open in the reading pane, swaps `GET /entries/{id}/summary/fragment` into `#rp-summary-container`.

The stream loops with a `select!` that races event delivery against the global `CancellationToken`, so SIGINT cleanly tears down all open SSE connections as part of graceful shutdown. `/events` is registered outside the ETag, Compression, Date-header, and Timeout middleware layers — those layers buffer or time-limit responses, which is fatal for a long-lived stream.

### Caching

In-memory, per-user caches reduce repeated work on hot read paths:

- **Sidebar cache** (`sidebar_cache.rs`) — caches the per-user sidebar chrome
  (feed/category tree and unread counts), replacing several SQL queries per
  request. It excludes session-specific fields (e.g. the masquerade admin flag)
  so one entry serves every request from the same `user_id`. It is bounded by
  capacity and a TTL, and is explicitly busted by handlers, background sync, and
  the summary worker whenever they change a user's feeds, entries, or counts.
  Because population is read-through — several `await`s against the DB before
  the value is stored — a bust landing inside that window would otherwise be
  overwritten by the older snapshot and hidden for the whole TTL. Each slot
  therefore carries a generation that `bust` bumps (leaving a tombstone rather
  than removing the slot); `read_chrome_data` takes the generation before its
  first read and publishes via `insert_if_current`, which drops the value if the
  generation moved. `RDRS_DISABLE_SIDEBAR_CACHE` turns the cache off entirely —
  set only by the E2E harness, which seeds straight into SQLite and so never
  runs the handlers carrying those bust hooks.
- **Page cache** (`page_cache.rs`) — a thin helper around `moka::sync::Cache`
  giving page handlers per-user, TTL-bounded caches for their rendered payloads.

### External Services

**Linkding** (`save/linkding.rs`):
- Saves entries to Linkding bookmark manager
- Configured per-user in settings

### AI Summarization

RDRS integrates with Kagi AI for automatic article summarization:

**Architecture:**
- `summarize/kagi.rs` - Kagi Universal Summarizer API client
- `summary_worker.rs` - Background worker for async processing
- `summary_cache.rs` - In-memory cache for summaries
- `summary_cleanup.rs` - Periodic cleanup of stale summaries

**Processing Flow:**
1. User requests summary for an entry
2. System checks cache, then database for existing summary
3. If not found, queues request to background worker
4. Worker calls Kagi API and stores result in `entry_summary` table
5. Summary is cached and returned to client

**Cancellation & Timeout:**
- Each in-flight or queued job has a `CancellationToken` in a shared `CancelRegistry` keyed by `(user_id, entry_id)`. A `POST /entries/{id}/summarize/cancel` request cancels the token (aborting the in-flight HTTP request) and deletes the record.
- Each Kagi request races against a 90-second hard timeout; on expiry the request is dropped and the summary is marked `failed`.

**Status Tracking:**
- Summaries track state: pending, processing, completed, failed
- Failed requests include error messages for debugging
- Cleanup task removes orphaned or expired summaries

## Security

### Signing & the root key (`secret.rs`)

One process-wide root key — `RDRS_SECRET`, or a random one at boot — backs every
signature rdrs produces. Each use derives its own tag through a
domain-separation prefix (`image:`, `greader-token:`, `session:`), so a value
minted for one purpose cannot be replayed as another. This matters concretely:
the CSRF token (added on top of this module) derives from the session token too,
and without the prefixes the token embedded in every rendered form *would be* the
session cookie's signature.

- **Session cookie.** The cookie value is `<session_token>.<hmac>`. Every
  extractor and the forward-auth middleware read it through
  `session_token_from_jar`, which verifies the signature before the token
  reaches the database — a forged or tampered cookie costs one HMAC, not a
  query, and a leaked `session.session_token` is useless without the root key.
- **Image-proxy URLs** and the **GReader post token** derive from the same key
  under their own domains (`image_proxy.rs`, `handlers/greader/auth.rs`).

Rotating the key — including the implicit rotation of a restart with no
`RDRS_SECRET` set — invalidates every signature at once: sessions end, and
image-proxy URLs already cached by a GReader client break until it re-syncs.
Native GReader `ClientLogin` tokens are unaffected, being the raw
`session_token` matched against the database rather than signed.

### CSRF protection

Two independent lines, so a bypass of one is not a bypass of both.

- **First line — `middleware::csrf::csrf_origin_guard`** (in place). A
  header-only, stateless check layered over the whole router. On every
  state-changing method it rejects a request the browser reports as cross-site
  (`Sec-Fetch-Site: cross-site`) or that carries an `Origin` whose host does not
  match the request's `Host` (an opaque `Origin: null` counts as cross-site).
  Requests with neither header — native GReader clients, `curl`, server-to-server
  calls, all bearer-authenticated rather than cookie-authenticated — are not a
  CSRF vector and pass through. `Sec-Fetch-Site` is trusted first; the `Origin`
  path compares host only, so it survives a TLS-terminating proxy where the
  browser's `https://` `Origin` meets a scheme-less forwarded `Host`.
- **Second line — synchronizer token** (`middleware::csrf::csrf_guard`, in
  place). A per-session token, `secret::derive_csrf` = HMAC of the session token
  under the `csrf:` domain, so it needs no column and no query and cannot equal
  the session cookie's own signature. On every unsafe method `csrf_guard`
  re-derives the expected token from the signed session cookie (no DB round trip)
  and requires the request to echo it — via the `X-CSRF-Token` header or, failing
  that, a `_csrf` urlencoded form field (the body is buffered and rebuilt so the
  handler still reads it). The token reaches the page as a readable (non-`HttpOnly`)
  `csrf_token` cookie, which `static/js/csrf.js` copies onto same-origin `fetch`
  requests and into native POST forms; the cookie is never trusted as the
  credential, only the derived MAC is. `multipart/form-data` is passed through and
  self-validated by the OPML-import handler (which also accepts `X-CSRF-Token`);
  the GReader prefixes are skipped (bearer-authenticated). A request with **no**
  session cookie is passed through, not rejected — a forged authenticated action
  must ride the victim's cookie, and login-CSRF is already caught by the first
  line.
- **Anonymous sessions** (`middleware::csrf::anonymous_session`). So the login
  and register forms carry a token before any real session exists, a logged-out
  visitor to an HTML page receives a signed `session_token` cookie that backs no
  `session` row (`find_by_token` finds nothing, so they stay unauthenticated)
  plus its `csrf_token`. Layered inside `forward_auth` so a real forward-auth
  session's `Set-Cookie` wins; skipped for `/api`, `/static`, `/favicon`,
  `/health`, and the GReader prefixes so shared caches are never cookie-poisoned.

### Password Hashing

Uses Argon2id with:
- Memory: 19 MiB
- Iterations: 2
- Parallelism: 1

### Session Management

- Sliding session expiry: 7-day TTL, extended on each authenticated
  request when less than half the TTL remains
- Absolute cap of 90 days from session creation to bound session lifetime
- Periodic token rotation (OWASP "Renewal Timeout") rides on that same
  trigger, so a token lives ~3.5 days instead of the up-to-90 the absolute cap
  would otherwise allow, with no extra column or timer to pace it. The
  extractor only *requests* the rotation (`middleware::auth::RotationSlot`);
  `slide_session_cookie` performs it on the way out, once it knows the response
  is one a cookie may ride on — a publicly cacheable response (the feed-icon
  route authenticates like any other but is served `public, max-age=…`) skips
  both the rotation and the reissue, since renaming a session whose new name
  never reaches the client would sign that client out. `rotate_token` matches
  on the old token, so concurrent requests cannot chain rotations: the first
  wins, the rest get `None` and keep the token they hold
- The replaced token stays valid for `ROTATION_GRACE_SECONDS` (60s) via
  `session.previous_token`, so requests already in flight when a rotation lands
  are not signed out. The grace arm lives in `find_by_token`, so every
  authenticated path inherits it; `delete_session` and
  `delete_user_sessions_except` match it too, or a logout arriving on the
  pre-rotation cookie would delete nothing
- Secure cookie settings (`HttpOnly`, `SameSite=Lax`); cookie `Max-Age`
  matches the absolute cap so the browser retains it across slides
- `Secure` is set from `Config::cookie_secure`, derived from `RDRS_PUBLIC_BASE_URL`'s
  scheme and overridable via `RDRS_COOKIE_SECURE`. Every login path (password,
  passkey, forward-auth) builds its cookie through
  `middleware::auth::build_session_cookie` so the attributes cannot drift apart
- **Re-authentication for credential changes** (OWASP "Reauthentication After
  Risk Events"). `session.last_authenticated_at` records when the session last
  *proved* itself rather than merely presented a cookie; the
  `middleware::auth::RecentlyAuthenticated` extractor requires it to be within
  `REAUTH_WINDOW_MINUTES` (5) and guards passkey registration and removal.
  A passkey is the one credential a password change does **not** revoke, so a
  picked-up session must not be able to add one silently.
  `POST /api/session/reauth` re-opens the window against the account password
  and shares the `PasswordChange` rate-limit budget, so it cannot be used to
  sidestep that limit. The check sits on the *start* of the registration
  ceremony because the challenge is single-use — a refusal at the finish would
  consume it and leave the retry with nothing to complete. Forward-auth
  sessions are exempt: the proxy re-asserts their identity every request, and
  accounts it creates hold a deliberately unverifiable password hash, so the
  window could never be reopened for them. A missing `last_authenticated_at`
  counts as stale, never fresh
- Masquerade feature for admin testing. Both transitions (start and stop) rotate
  the session token in the same `UPDATE` that swaps `user_id`, since entering or
  leaving a masquerade is a privilege-level change and OWASP's Session
  Management Cheat Sheet requires the session ID to be renewed across one. The
  handlers reissue **both** the session cookie and the CSRF cookie from the new
  token — the latter's value is derived from the former (`secret::derive_csrf`),
  so a client left holding the old CSRF token would fail every subsequent
  state-changing request

### Input Sanitization

- All HTML content sanitized with Ammonia
- SQL injection prevented via parameterized queries throughout (including dynamic filter conditions)
- SSRF protection in readability fetcher and image proxy (shared validation module)

## Deployment

### Docker

Multi-stage Dockerfile:
1. **chef** - Install cargo-chef for caching
2. **planner** - Generate dependency recipe
3. **builder** - Compile application
4. **runtime** - Distroless base image

Benefits:
- Small image size (~50MB)
- Minimal attack surface
- Layer caching for fast rebuilds

### Production Considerations

Deployment and production configuration (image proxy secret, `/data`
persistence, TLS via a reverse proxy) are documented in
[README.md → Production Notes](README.md#production-notes).
