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
│   ├── schema.rs        # SQLite schema initialization
│   └── pool.rs          # Priority-based database connection pool
│
├── models/              # Data models and database operations
│   ├── user.rs          # User accounts
│   ├── session.rs       # Session management
│   ├── feed.rs          # RSS feeds
│   ├── entry.rs         # Feed entries
│   ├── entry_summary.rs # Article summaries
│   ├── category.rs      # Feed categories
│   ├── image.rs         # Image storage
│   ├── statistics.rs    # Statistics/analytics queries
│   ├── passkey.rs       # WebAuthn credentials
│   ├── webauthn_challenge.rs # WebAuthn challenge state
│   └── user_settings.rs # User preferences
│
├── handlers/            # HTTP request handlers
│   ├── pages.rs         # HTML page rendering
│   ├── auth.rs          # Authentication endpoints
│   ├── passkey.rs       # Passkey/WebAuthn endpoints
│   ├── admin.rs         # Admin operations
│   ├── user.rs          # User operations
│   ├── category.rs      # Category CRUD
│   ├── feed.rs          # Feed CRUD
│   ├── entry.rs         # Entry operations
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
│   ├── events.rs        # In-memory EventBus for SSE live updates
│   ├── feed_sync.rs     # Feed refresh logic
│   ├── feed_discovery.rs# Feed URL detection
│   ├── readability.rs   # Content extraction
│   ├── sanitize.rs      # HTML sanitization
│   ├── opml.rs          # OPML import/export
│   ├── icon_fetcher.rs  # Feed icon fetching
│   ├── http.rs          # Shared HTTP client utilities
│   ├── image_proxy.rs   # Secure image proxying
│   ├── summary_cache.rs # Summary caching
│   ├── summary_cleanup.rs # Summary cleanup task
│   ├── summary_worker.rs# Summary generation worker
│   ├── save/
│   │   └── linkding.rs  # Linkding integration
│   └── summarize/       # AI summarization
│       ├── mod.rs       # Summarizer trait
│       └── kagi.rs      # Kagi AI service
│
├── middleware/          # HTTP middleware
│   ├── auth.rs          # Session authentication
│   ├── date_header.rs   # Date response header
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
- `DATABASE_URL` - SQLite file path
- `SERVER_PORT` - HTTP port
- `SIGNUP_ENABLED` / `MULTI_USER_ENABLED` - Registration settings
- `IMAGE_PROXY_SECRET` - HMAC secret for image proxy
- `AUTH_PROXY_HEADER` - Header name carrying the username from a forward-auth proxy; empty disables the feature
- `TRUSTED_PROXY_NETWORKS` - Comma-separated CIDRs/IPs whose TCP peer is allowed to supply the identity header; required when `AUTH_PROXY_HEADER` is set
- `AUTH_PROXY_USER_CREATION` - Whether to JIT-create an account for an unknown proxy-provided username (`false` by default; on mismatch, redirects to `/login`)
- `AUTH_PROXY_GROUPS_HEADER` - Header name carrying comma-separated group names from the proxy
- `AUTH_PROXY_ADMIN_GROUP` - Group membership grants the admin role; active only when both this and `AUTH_PROXY_GROUPS_HEADER` are set
- `DISABLE_LOCAL_AUTH` - Hides the browser password form and makes `POST /api/session` return 403; does not affect GReader `ClientLogin` or passkey auth; startup refuses if set without `AUTH_PROXY_HEADER`

### Error Handling (`error.rs`)

Custom `AppError` type that maps to appropriate HTTP responses:
- Authentication errors → 401
- Not found → 404
- Validation errors → 400
- Internal errors → 500

## Data Layer

### Database (`db/schema.rs`)

Schema migrations are tracked using `PRAGMA user_version`. Each migration runs once and advances the version number, replacing the previous ad-hoc `ALTER TABLE` approach.

SQLite schema with 10 tables:

| Table | Purpose |
|-------|---------|
| `user` | User accounts with role (admin/user) |
| `session` | Session tokens with masquerade support |
| `category` | Feed categories per user |
| `feed` | Feed metadata with etag caching and bucket assignment |
| `entry` | Feed items with read/starred status |
| `entry_summary` | AI-generated article summaries |
| `image` | Polymorphic image storage |
| `user_settings` | User preferences and service configs |
| `passkey` | WebAuthn credential storage |
| `webauthn_challenge` | WebAuthn challenge state |

### Connection Pool (`db/pool.rs`)

`DbPool` manages two SQLite connections under WAL mode:

- **Write connection** - Handles all INSERT/UPDATE/DELETE operations via `user()` and `background()` methods
- **Read-only connection** - Handles SELECT queries via `read_user()` and `read_background()` methods, with `PRAGMA query_only=ON` for safety

Both connections use priority-based scheduling: user requests are always processed before background tasks (e.g., feed sync).

### Models

Each model provides:
- Struct definition matching database schema
- CRUD operations as associated functions (using params structs like `CreateFeedParams` to avoid excessive positional arguments)
- Query methods for common access patterns

Example: `Feed` model provides `find_by_user`, `create`, `update`, `delete`, `find_due_for_sync`.

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
4. Sets session cookie
5. Subsequent requests extract user from `AuthUser` extractor

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

**Trust model:**

The middleware checks the TCP peer IP of the incoming connection against a set of trusted CIDRs/IPs (`TRUSTED_PROXY_NETWORKS`). The peer address comes from the connection itself (`ConnectInfo`), not from `X-Forwarded-For`, so it cannot be spoofed by a downstream client. If the peer is untrusted, the identity header is ignored and the request proceeds to the normal session-cookie check. The middleware fails closed: any of untrusted peer, missing header, absent `ConnectInfo`, or DB error leaves the user unauthenticated.

**Username mapping (no schema change):**

The proxy-provided username is matched against existing rdrs accounts by username. No database migration is required. Existing password accounts continue to work and automatically gain forward-auth login when their username matches the proxy-provided value.

**JIT account creation:**

When `AUTH_PROXY_USER_CREATION=true`, a proxy-provided username that matches no existing account causes a new local account to be created with a sentinel password hash (`"!"`) that cannot match any real password input, making local password login impossible for that account.

**Group → role sync:**

When both `AUTH_PROXY_GROUPS_HEADER` and `AUTH_PROXY_ADMIN_GROUP` are set, the user's role is recomputed from the groups header on every forward-auth login and persisted if it changed. The proxy/IdP is authoritative for role assignment while this mapping is active.

**`DISABLE_LOCAL_AUTH` scope:**

Setting `DISABLE_LOCAL_AUTH=true` hides the browser password-entry form and makes `POST /api/session` return HTTP 403. It does **not** affect GReader `ClientLogin` (`/accounts/ClientLogin`) or WebAuthn/passkey authentication, so native RSS clients and passkey users are unaffected.

**Middleware scope:**

The middleware is applied only to browser page routes. It is never invoked for the prefixes `/api`, `/reader`, `/accounts`, `/events`, `/static`, `/favicon`, and `/health`. It also skips requests that already carry a valid session cookie, so it adds no overhead for already-logged-in users.

**Forward and passkey auth coexist:**

Forward-auth, local password, and passkey authentication all work simultaneously by default. `DISABLE_LOCAL_AUTH` is the only knob that narrows that set.

**Operator warnings:**

1. The reverse proxy **must** authoritatively set (and strip any client-supplied copy of) the identity and groups headers on every request before forwarding to RDRS. A downstream client that can inject these headers bypasses the trust model entirely.
2. The reverse proxy **must** be configured to bypass forward-auth for `/accounts/ClientLogin` and `/reader/api/...` so native GReader clients (FeedMe, Read You, etc.) can still authenticate with their stored username and password.

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
  - Backward compatible behavior when `PUBLIC_BASE_URL` is not configured
- **Absolute URLs** (optional): `https://rdrs.example.com/api/proxy/image?url=...&s=...`
  - Used by Google Reader API when `PUBLIC_BASE_URL` is configured
  - Required for native RSS clients (e.g., NetNewsWire) that render HTML directly
  - Configured via `PUBLIC_BASE_URL` environment variable

### SSE Live Updates

A single `GET /events` endpoint (`handlers/events.rs`) streams per-user Server-Sent Events to each open browser tab. Mutation paths (mark-read, mark-unread, mark-all, summarize, etc.) call `EventBus::emit_sidebar` or `emit_summary` on the shared in-memory `EventBus` (`services/events.rs`), which is a thin wrapper over a `tokio::sync::broadcast` channel. The browser's `EventSource` (wired up in `installSse()` in `static/js/app.js`) handles two event types:

- **`sidebar`** — triggers a notify-and-fetch refresh of `/api/sidebar`, updating the unread/summarized badge counts without a page reload.
- **`summary`** — carries `{entry_id, status}` JSON; the client rewrites the entry-row badge and, if that entry is open in the reading pane, swaps `GET /entries/{id}/summary/fragment` into `#rp-summary-container`.

The stream loops with a `select!` that races event delivery against the global `CancellationToken`, so SIGINT cleanly tears down all open SSE connections as part of graceful shutdown. `/events` is registered outside the ETag, Compression, Date-header, and Timeout middleware layers — those layers buffer or time-limit responses, which is fatal for a long-lived stream.

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

### Password Hashing

Uses Argon2id with:
- Memory: 19 MiB
- Iterations: 2
- Parallelism: 1

### Session Management

- Sliding session expiry: 7-day TTL, extended on each authenticated
  request when less than half the TTL remains
- Absolute cap of 90 days from session creation to bound session lifetime
- Secure cookie settings (`HttpOnly`, `SameSite=Lax`); cookie `Max-Age`
  matches the absolute cap so the browser retains it across slides
- Masquerade feature for admin testing

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

- Set `IMAGE_PROXY_SECRET` for persistent image URLs
- Mount `/data` volume for database persistence
- Consider reverse proxy for TLS termination
