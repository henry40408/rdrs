# Architecture

RDRS is a self-hosted RSS reader built with Rust.

## Overview

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
│           Database (SQLite / PostgreSQL)            │
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
├── secret.rs            # Keyed derivation from RDRS_SECRET (domain-separated)
├── test_support.rs      # Shared unit-test fixtures
│
├── db/
│   └── pool.rs          # Dual-backend (SQLite/PostgreSQL) sqlx pool + SQLite write-priority scheduler
│
├── models/              # Data models and database operations
│   ├── user.rs          # User accounts
│   ├── api_token.rs     # GReader ClientLogin tokens (separate from sessions)
│   ├── session.rs       # Session management
│   ├── feed.rs          # RSS feeds
│   ├── entry/           # Feed entries (mod.rs + filters.rs query builder + query.rs boolean search parser)
│   ├── entry_summary.rs # Article summaries
│   ├── entry_open.rs    # Rendered-entry opens and per-feed open rate
│   ├── category.rs      # Feed categories
│   ├── image.rs         # Image storage
│   ├── statistics.rs    # Statistics/analytics queries
│   ├── passkey.rs       # WebAuthn credentials
│   ├── user_invite.rs   # One-time account-activation / password-reset links
│   ├── webauthn_challenge.rs # WebAuthn challenge state
│   └── user_settings.rs # User preferences
│
├── handlers/            # HTTP request handlers
│   ├── pages/           # HTML page rendering (mod.rs + script_json/search_text/time_format helpers)
│   ├── auth.rs          # Authentication endpoints
│   ├── passkey.rs       # Passkey/WebAuthn endpoints
│   ├── invite.rs        # Anonymous redemption of a one-time account link
│   ├── admin.rs         # Admin operations
│   ├── user.rs          # User operations + sidebar payload
│   ├── categories.rs    # Category form actions (SSR)
│   ├── feeds.rs         # Feed form actions: create/edit/delete/refresh/OPML import (SSR)
│   ├── feed.rs          # Per-feed JSON endpoints (e.g. icon)
│   ├── entries.rs       # Entry SSR fragments + form actions (read/star/summarize/save)
│   ├── entry.rs         # Per-entry JSON endpoints (summary, neighbors, full content)
│   ├── favicon.rs       # Favicon serving (embedded at compile time)
│   ├── static_assets.rs # Static assets, the web app manifest and `/sw.js` (embedded at compile time)
│   ├── proxy.rs         # Image proxy
│   ├── health.rs        # Health check endpoint
│   ├── offline.rs       # Offline-reading manifest
│   ├── pixel.rs         # Open-tracking pixel endpoint
│   ├── summarizer.rs    # Batch URL summarizer
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
│   ├── han.rs           # Simplified-Chinese detection for the `lang` attribute
│   ├── http.rs          # Request-header helpers (User-Agent)
│   ├── text.rs          # HTML to plain text
│   └── url_validation.rs# URL validation and SSRF protection
│
├── services/            # Business logic
│   ├── background.rs    # Background sync scheduler
│   ├── audit.rs         # Session/credential audit events (`rdrs::audit`)
│   ├── content_text_backfill.rs # One-shot `entry.content_text` backfill
│   ├── entry_retention.rs # Read-entry retention/pruning worker
│   ├── events.rs        # In-memory EventBus for SSE live updates
│   ├── feed_sync.rs     # Feed refresh logic
│   ├── feed_discovery.rs# Feed URL detection
│   ├── fetch.rs         # SSRF-guarded HTTP client for untrusted URLs
│   ├── readability.rs   # Content extraction
│   ├── sanitize.rs      # HTML sanitization
│   ├── html_entities.rs # HTML entity decoding for plain-text fields
│   ├── opml.rs          # OPML import/export
│   ├── icon_fetcher.rs  # Feed icon fetching
│   ├── http.rs          # Shared HTTP client utilities
│   ├── image_proxy.rs   # Secure image proxying
│   ├── page_cache.rs    # Per-user TTL page-payload caches (moka)
│   ├── pixel.rs         # Open-tracking pixel injection
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
│   ├── cache_control.rs # `Cache-Control: no-store` for session responses
│   ├── csrf.rs          # CSRF defence (origin check + token)
│   ├── date_header.rs   # Date response header
│   ├── etag.rs          # ETag / conditional-request handling
│   ├── flash.rs         # Flash messages
│   ├── forward_auth.rs  # Forward-auth / trusted-header browser login
│   ├── rate_limit.rs    # Credential-endpoint rate limiting
│   ├── request_log.rs   # Per-request duration logging
│   └── security_headers.rs # CSP, HSTS and other security headers
│
└── auth/
    ├── password.rs      # Password hashing (Argon2)
    └── webauthn.rs      # WebAuthn/Passkey authentication

templates/               # Askama HTML templates
tests/                   # Integration tests
```

## Core Components

### Application Entry (`main.rs`)

Initializes config, database, background tasks, and the Axum server.

### Router (`lib.rs`)

Defines all routes; builds the app with embedded static assets
(`include_str!`/`include_bytes!`), the cookie layer, and the DB pool as state.

**Asset cache invalidation.**
- Templates stamp every embedded asset URL with `?v={{ git_version }}`.
- Only stamped requests get `public, max-age=31536000, immutable`
  (`static_assets::cache_control_for`; favicons via `cache_control_for_request`).
  Unstamped requests (`/favicon.ico` probes, `/apple-touch-icon.png`, bare
  module imports) get a short TTL.
- Nested JS imports get the stamp by substituting `__RDRS_ASSET_VERSION__` at
  serve time.
- A `-dirty` build serves `no-cache` throughout (its version string does not
  change between edits).

### Configuration (`config.rs`)

Loads settings from environment variables (full table in
[README.md → Configuration](README.md#configuration)):
- `DATABASE_URL` - file path / `sqlite://` (SQLite, default) or `postgres://`; chosen once at startup
- `SERVER_PORT` - HTTP port
- `RDRS_MULTI_USER_ENABLED` - whether an admin may create accounts beyond the first; no self-service sign-up (`RDRS_SIGNUP_ENABLED` is retired and refuses startup)
- `RDRS_SECRET` - root HMAC key for every signature, domain-separated in `secret.rs`
- `RDRS_AUTH_PROXY_HEADER` - forward-auth username header; empty disables
- `RDRS_TRUSTED_PROXY_NETWORKS` - CIDRs/IPs whose TCP peer may supply the identity header; required with `RDRS_AUTH_PROXY_HEADER`
- `RDRS_AUTH_PROXY_USER_CREATION` - JIT-create unknown proxy users (default `false`; otherwise redirect to `/login`)
- `RDRS_AUTH_PROXY_GROUPS_HEADER` - comma-separated groups header
- `RDRS_AUTH_PROXY_ADMIN_GROUP` - group granting admin; active only with the groups header
- `RDRS_DISABLE_LOCAL_AUTH` - hides the password form, `POST /api/session` → 403; GReader `ClientLogin` and passkeys unaffected; refuses startup without `RDRS_AUTH_PROXY_HEADER`
- `RDRS_AUTH_PROXY_LOGOUT_URL` - Sign Out redirects here to end the IdP session; if unset, forward-auth Sign Out clears the local session and warns the user to log out at the proxy

### Error Handling (`error.rs`)

`AppError` maps to HTTP: auth → 401, not found → 404, validation → 400, internal → 500.

## Data Layer

### Schema & Migrations (`migrations/`)

Embedded per backend (`migrations/sqlite/`, `migrations/postgres/`), run at
startup via `sqlx::migrate!`. Schemas are equivalent; dialect differences
(`BIGINT` for non-id integers, `GENERATED ALWAYS AS IDENTITY`) live in the
Postgres migrations.

| Table | Purpose |
|-------|---------|
| `user` | User accounts with role (admin/user) |
| `session` | Session tokens: masquerade support, `previous_token`/`previous_token_expires_at` rotation grace, `last_authenticated_at` for re-auth, `user_agent`/`ip_address`/`last_seen_at` |
| `category` | Feed categories per user |
| `feed` | Feed metadata with etag caching and bucket assignment |
| `entry` | Feed items with read/starred status |
| `entry_summary` | AI-generated article summaries |
| `entry_tombstone` | Retention-pruned entries; blocks re-insertion on sync (cascades with feed) |
| `image` | Polymorphic image storage |
| `user_settings` | User preferences and service configs |
| `passkey` | WebAuthn credential storage |
| `user_invite` | One-time links that set an account's password (HMAC-stored) |
| `webauthn_challenge` | WebAuthn challenge state |

### Connection Pool (`db/pool.rs`)

- `struct Db` wraps `enum DbInner { Sqlite(SqlitePool), Postgres(PgPool) }`,
  selected by `DATABASE_URL`.
- All queries go through `query_*!` / `db_execute!`, so SQL and binds are
  written once. Dialect differences live in `entry::filters::Dialect` and the
  `pg_rewrite` shim (`datetime('now')`→`now()`, `to_char` cursor comparisons,
  `make_interval`, quoted `"user"`).
- Exception: the entry upsert's NULL-safe inequality (`IS NOT` /
  `IS DISTINCT FROM`) is a hand-dispatched `UPSERT_UPDATE_SQL_SQLITE` / `_PG`
  pair in `models/entry/mod.rs`, because a `pg_rewrite` rule for `IS NOT` would
  also rewrite every `IS NOT NULL`.
- PG connections pin `TimeZone=UTC` so timestamp-string cursors match SQLite.

**Planner statistics (SQLite only).** SQLite never refreshes `sqlite_stat1`
itself, and stale (or missing, for a new index) stats cost plans.
`Db::optimize()` (`PRAGMA optimize`) runs at the end of `connect()` and on every
retention tick after the first. `analysis_limit=400` bounds the ANALYZE per
connection. No-op on PostgreSQL.

**Write-priority scheduling (SQLite only).** SQLite has one writer under WAL.
`Db` carries a `Priority` (`User` by default; background workers call
`db.background()`) and a shared `SqliteSched` whose `admit()` holds a background
write until no `User` write is in flight. Reads are never gated. No-op on
PostgreSQL.

### Models

Each model has a struct matching the schema, CRUD associated functions (params
structs like `CreateFeedParams` instead of long argument lists), and query
methods (e.g. `Feed::find_by_user`, `find_due_for_sync`).

Global `/search` supports a boolean query language (`is:`, `feed:`,
`category:`, `title:`, `author:`, `before:`, `after:`, `AND`/`OR`/`NOT`,
grouping, quoting, `-` negation). `models/entry/query.rs` parses it into a
`QueryNode` AST on `EntryFilter.query`; `filters::render_query` renders it per
`Dialect` using `LIKE`/`ILIKE` (no full-text index). Scoped per-view search uses
the plain substring `EntryFilter.search`.

## HTTP Layer

### Handlers

Organized by resource: pages (HTML), auth, feed, entry, admin, GReader — see
[Directory Structure](#directory-structure).

**Bulk writes report what they changed**, using the database's affected-row
count, not the requested count.
- Form actions put it in the flash (`mark_read_scoped`, OPML import via
  `opml::ImportSummary::describe`, revoke-others, revoke-all-tokens).
- GReader `mark-all-as-read`, `edit-tag`, `subscription/import` must keep the
  bare `OK` body, so the count goes in `X-RDRS-Affected`
  (`handlers::greader::AFFECTED_HEADER`). `app.js` reads it, falling back to its
  DOM-row count.

### Middleware

- **auth.rs** - `AuthUser` from the session cookie; `AdminUser` for admin routes
- **flash.rs** - flash messages in cookies

### Account Creation

No self-service registration: an anonymous endpoint taking a username always
leaks whether it exists.

1. **`POST /api/setup`** creates the first account (admin), refused once
   `user::count() > 0` (`Config::can_setup`). `GET /setup` then redirects to
   `/login`.
2. **`POST /admin/users`** creates every later account with
   `password_hash = "!"` (unparseable PHC, same as `forward_auth`), so
   `verify_password` fails for all input, browser and GReader alike.
3. **`user_invite`** holds a one-time link. Token generated like a session
   token, stored as an HMAC under `secret::DOMAIN_INVITE`; raw value shown once
   in the admin's flash. Expires after `INVITE_TTL_DAYS` (7); re-issuing revokes
   the previous link.
4. **`GET`/`POST /invite/{token}`** is anonymous. Every failure (unknown,
   expired, spent) renders one identical page naming no account. Redemption
   spends the invite with `UPDATE … WHERE consumed_at IS NULL` *before* writing
   the password (so racing submissions can't both win), then clears the
   account's sessions and API tokens.

This is also the only password-reset path; the old password keeps working until
the link is redeemed.

### Authentication Flow

1. Credentials posted to `POST /api/session`
2. Argon2 verification. Unknown usernames still verify against a throwaway hash
   (`auth::verify_dummy_password`) so timing doesn't reveal account existence;
   the error message is generic
3. Session row created
4. Signed session cookie set (`<token>.<hmac>`, see [Signing & the root key](#signing--the-root-key-secretrs))
5. `AuthUser` verifies the signature before the DB lookup on later requests

### WebAuthn/Passkey Authentication

**Registration:**
1. User starts registration from settings
2. Server stores a challenge in `webauthn_challenge`, requiring a
   **discoverable** credential (`residentKey: required`) — sign-in is usernameless
3. Browser creates the passkey
4. Client sends attestation
5. Server validates and stores it in `passkey`

**Authentication:**
1. User clicks "Login with Passkey"
2. Server issues a challenge with **empty `allowCredentials`** and reads nothing
   from `passkey`. Load-bearing: listing stored credential IDs leaks stable
   per-user IDs, and differing on an empty table is an account-existence oracle
3. Browser verifies the passkey
4. Client sends the assertion
5. Server resolves the credential by ID, verifies against that key
   (`finish_discoverable_authentication`), and creates a session

> **Older passkeys may need re-registering.** They were created with
> `residentKey: discouraged`; authenticators that honoured it hold
> non-discoverable credentials that can't be offered without `allowCredentials`.
>
> - **Affected**: most security keys, some Windows Hello setups, password
>   managers that respect the hint (e.g. Bitwarden, which only answers an empty
>   `allowCredentials` for passkeys flagged `discoverable`).
> - **Unaffected**: iCloud Keychain, Google Password Manager.
>
> Recovery order matters: `start_registration` lists existing credential IDs in
> `excludeCredentials`, so the authenticator refuses a second one
> (`InvalidStateError`). Sign in with a password, **delete the old passkey** in
> `/user-settings`, then register. The failure is a client-side timeout
> ("Authentication was cancelled or timed out." in `login.js`) that never
> reaches the server, so it can't be detected.

### Forward-Auth (Trusted-Header) Login

`middleware/forward_auth.rs` establishes a session from a trusted identity
header set by a forward-auth proxy (Authelia, authentik, Traefik ForwardAuth),
falling back to normal cookie login.

> **Operator setup** — env vars, reverse-proxy requirements (header stripping, GReader and image-proxy bypass), logout behavior: [README.md → Authentication & SSO](README.md#authentication--sso).

- **Trust model:** the TCP peer IP (`ConnectInfo`, never `X-Forwarded-For`)
  must be in `RDRS_TRUSTED_PROXY_NETWORKS`; otherwise the header is ignored.
  Fails closed on untrusted peer, missing header, missing `ConnectInfo`, or DB
  error.
- **Username mapping:** matched against existing usernames; no schema change.
  Existing password accounts gain forward-auth login automatically.
- **JIT creation:** with `RDRS_AUTH_PROXY_USER_CREATION=true`, unknown users get
  an account with sentinel hash `"!"` (no password login possible).
- **Group → role sync:** with both `RDRS_AUTH_PROXY_GROUPS_HEADER` and
  `RDRS_AUTH_PROXY_ADMIN_GROUP`, role is recomputed and persisted on every
  forward-auth login; the IdP is authoritative.
- **`RDRS_DISABLE_LOCAL_AUTH`:** hides the password form, `POST /api/session` →
  403. GReader `ClientLogin` (`/accounts/ClientLogin`) and passkeys unaffected.
- **Scope:** browser page routes only; never `/api`, `/reader`, `/accounts`,
  `/events`, `/static`, `/favicon`, `/health`. Skipped when a valid session
  cookie is present.

**Logout mechanics:**
- Sign Out always clears `session_token` (`Path=/`) and deletes the server
  session. It succeeds even if the session is already gone (idle timeout,
  revocation, new `RDRS_SECRET`): `DELETE /api/session` and `POST /logout` still
  clear cookies and send `Clear-Site-Data` rather than 401.
- Forward-auth re-authenticates whenever there is no *valid* session cookie, so
  a stale cookie never causes a lockout. `/login` redirects authenticated users
  to `/`.
- Consequently a local logout can't end a forward-auth session without
  `RDRS_AUTH_PROXY_LOGOUT_URL`. `DELETE /api/session` returns
  `logout_url_configured`, `via_forward_auth`, and `redirect_to` (absolute IdP
  URL or same-host path). `rdrs-sidebar.js` redirects and shows "You have been
  logged out." normally; for forward-auth without a logout URL it stays put and
  warns the user to log out at the proxy/SSO provider.

**Active session list:**
- `/user-settings` lists non-expired sessions as cards (`.cred-list` /
  `.cred-card`, shared with the API token list): full `user_agent`,
  `ip_address`, created/last-active/expires. The row `id` reaches the template;
  `session_token` never does.
- `POST /user-settings/sessions/{id}/revoke` —
  `session::delete_user_session_by_id`, `user_id`-scoped, reports "already
  gone" from the affected-row count. Refuses the caller's own session (card
  shows "This device"; handler re-checks).
- `POST /user-settings/sessions/revoke-others` —
  `session::delete_user_sessions_except`; count goes to the flash and to
  `audit::sessions_destroyed_bulk`'s `count`.
- `POST /user-settings/api-tokens/revoke-all` (`api_token::delete_user_tokens`)
  works the same way.

**Session metadata:**
- `user_agent`, `ip_address`, `last_seen_at` are `NOT NULL`, captured at all 4
  login sites (`POST /api/session`, forward-auth, passkey
  `finish_authentication`, GReader `ClientLogin`).
- Client IP via `Config::client_ip`: honours `X-Forwarded-For`/`X-Real-IP` only
  when the TCP peer is trusted (`is_trusted_peer`, same as forward-auth). Reads
  `X-Forwarded-For` right-to-left, taking the right-most entry that isn't a
  trusted proxy (append-mode proxies add hops on the right; left-most would be
  client-spoofable), then `X-Real-IP`, then the peer.
- `last_seen_at` is bumped by `AuthUser`/`PageAuthUser`, throttled to once per
  minute per session (`session::touch_last_seen`).
- Migration `0002_add_session_metadata` **drops and recreates `session`**:
  upgrading signs everyone out.

**Auth-mode indicator:** the sidebar shows an **SSO** pill when the request came
via forward-auth (computed per request; `via_forward_auth` on extractors and the
sidebar payload). `/settings` shows forward-auth config; it is **admin-only**
(`PageAdminUser`), passes `DATABASE_URL` through `config::redact_database_url`,
and the sidebar hides its link for non-admins.

## Services

### Feed Synchronization

**Background Scheduler** (`background.rs`): a Tokio task that assigns feeds to
60 one-minute buckets by URL hash (indexed `bucket` column) and syncs the
current bucket every minute.

**Sync Logic** (`feed_sync.rs`): etag / if-modified-since; feed-rs with a
custom timestamp parser (Chinese dates); inserts new entries, skips duplicates;
`JoinSet` with concurrency 4.

**Timestamps on `/feeds`:**
- `fetched_at` — written on every attempt.
- `feed_updated_at` — `effective_feed_updated_at(feed_timestamp,
  latest_entry_date, http_last_modified)`: the **max** of the present values
  (`.flatten().max()`), so a feed with only `Last-Modified` isn't called stale.
  Feed timestamp is `updated.or(published)`; entry date is
  `published.or(updated)`, falling back to the feed timestamp. Written via
  `COALESCE($5, feed_updated_at)`, so a `304` or failure never clears it.
- `compute_freshness` (`handlers/pages/time_format.rs`) grades against
  `FRESH_MAX_DAYS` / `WARNING_MAX_DAYS`, falling back to `fetched_at`. The Stale
  filter matches the stale band only. The constants are passed into
  `feeds.html` (which explains the rule in a `<details>`) rather than retyped.
- No failure counter or auto-disable: `fetch_error` holds the last error until
  the next success.

### Entry Retention

**Retention Worker** (`entry_retention.rs`):
- Opt-in per user via `user_settings.retention_read_days` (`0` = off).
- Every 24h, prunes read, non-starred entries older than the window, in batches.
- Each pruned entry gets an `entry_tombstone` (`feed_id`, `guid`) so sync won't
  re-insert it; tombstones cascade with the feed.
- SQLite maintenance: a tick that pruned runs `run_maintenance` (planner stats,
  VACUUM when the freelist reaches 20% of the file, truncating WAL checkpoint);
  a tick that pruned nothing still refreshes stats. The first (immediate) tick
  skips it since `Db::connect` just did.

### Content Processing

**HTML Sanitization** (`sanitize.rs`):
- Ammonia for XSS protection (always runs, ungated)
- Drops `aria-hidden="true"` subtrees first (Ammonia strips `class`/`style`, so
  hidden-by-CSS markup like Shiki line-number gutters would surface as text);
  falls back to the original if that empties the entry
- Strips tracking params (utm_*, fbclid, …), tracking domains (pixel.*,
  analytics.*, …), and 1x1 pixels
- Fixes relative image URLs
- Injects `width`/`height` from `data-original-width`/`-height` or inline
  `style` when missing
- Tags proxied images `data-img-state="loading"` (CSS skeleton; broken-image
  fallback with `alt` on error)
- The three pre-Ammonia passes (`aria-hidden`, lazy-image promotion, dimension
  harvesting) are each gated by a cheap case-insensitive substring test
  returning a borrowed `Cow`. **A gate must remain a superset of its pass's
  trigger**; `gates_are_supersets_of_the_passes_they_front` enforces this. The
  passes are presentation, not security

**Full Content Extraction** (`readability.rs`):
- Fetches the article and extracts main content; SSRF-guarded via
  `utils/url_validation`
- Stored **raw** in `entry.full_content` and sanitized per render (sanitizing
  signs proxy URLs with `RDRS_SECRET`, which a stored copy would outlive)
- Persists across refreshes/tabs/no-JS; `GET /entries/{id}/fragment?view=original`
  shows the feed's version; re-posting the action refreshes it
- Not mirrored into `content_text` (search covers feed content only)

### Image Proxy (`image_proxy.rs`)

Proxies external images for privacy and to avoid mixed content.
- Signed URLs: `/api/proxy/image?url=...&s=...`, HMAC-SHA256 truncated to 8
  bytes, verified before fetching.
- `Cache-Control: public, max-age=86400`, `ETag` = the signature; matching
  `If-None-Match` → `304` without refetching (checked *after* signature
  verification).
- `GET /api/feeds/{id}/icon` uses `private, max-age=86400`: it is per-user
  (`AuthUser`, scoped to the caller's categories) and sets its own
  `Cache-Control`, so `no_store_for_authenticated` adds no `Vary: Cookie`;
  `public` would leak icons across users via shared caches. The proxy stays
  `public` because its URLs carry no per-user meaning.

**URL format:**
- **Relative** (default): used by the web UI and when `RDRS_PUBLIC_BASE_URL` is unset.
- **Absolute** (`https://rdrs.example.com/api/proxy/image?...`): used by the
  GReader API when `RDRS_PUBLIC_BASE_URL` is set; needed by native clients
  (e.g. NetNewsWire) that render HTML directly.

### Partial swaps (`data-swap`)

`installSwap()` (`static/js/app.js`) intercepts clicks/submits on
`data-swap="<selector>"` elements, fetches, and replaces the target or every
`<template data-swap-target="…">` in the response. Non-2xx falls back to real
navigation. Entries-family response shapes:

- **reading pane** — `GET /entries/{id}/fragment` replaces `#reading-pane` (and
  the row via multi-target templates). Prev/Next come from the DOM when the
  entry has rows on both sides; otherwise from `GET /api/entries/{id}/neighbors`
  (list ends, which may have more pages or an `after` cursor, and scoped
  searches, which `NeighborsQuery` can't express).
- **Load More** — `?fragment=1&after=<cursor>` appends before `#load-more`. Page
  size is the user's `entries_per_page` via `entries_page_size`, read per
  request and clamped to `MIN..=MAX_ENTRIES_PER_PAGE` (it reaches a SQL `LIMIT`).
- **list refresh** — `?fragment=1` (no cursor) re-renders `[data-entries-list]`
  and the "Mark N matching" slot, leaving the search box alone. The search
  drawer sits above `.list-pane-header` and renders open when `q` is present.
  Searching hides "Mark Above as Read" (the `A` shortcut still works). Also
  backs **Mark Above as Read** and the **"Mark as Read..." dropdown** via
  `refreshEntriesList()`, which re-renders in place and raises
  `rdrs:sidebar-stale` instead of reloading. `/` distinguishes
  `?fragment=1&after=` from bare `?fragment=1`.
- **sidebar navigation** — `?pane=1` (`/categories/{id}/entries`,
  `/feeds/{id}/entries`) replaces `[data-list-pane]` and resets
  `#reading-pane`. `swapListPane()` drives it for sidebar links, `[` `]` `{` `}`,
  and `g c` / `g f`; it `pushState`s the URL and patches `.active` classes
  without re-rendering `<rdrs-sidebar>`. `popstate` swaps back for
  category/feed paths, reloads otherwise.

**Sidebar feeds:**
- The open category's feeds load on demand from
  `GET /api/sidebar/categories/{id}/feeds` (with unread counts), cached per
  category in `sessionStorage`. Kept out of `/api/sidebar`, which is embedded
  in every `no-store` page. Revalidated on `?pane=1` and `rdrs:sidebar-stale`.
- `[` / `]` (and unread-only `{` / `}`) walk categories and open-category feeds
  as one flat list in display order.
- `user_settings.sidebar_sort` (`name` | `unread`) and
  `user_settings.sidebar_hide_read` ride in `/api/sidebar` and are applied
  client-side in `arrangeSidebarRows()`, since only the client knows the active
  row — which stays listed at zero unread. Unread ordering settles on full
  render only; `isStructuralChange()` treats a changed visible set as
  structural, a changed order as not.
- With `sidebar_hide_read`, ordinary mark-as-read is structural, so `render()`
  preserves the open category's feed rows (re-adopted by id so favicons aren't
  recreated) and `.sidebar-nav`'s scroll offset across its `innerHTML` rebuild.

**Staleness check:** non-navigation swaps into `#reading-pane` or
`#rp-summary-container` drop the markup (applying only the flash) if the pane
no longer shows the request's entry — otherwise an SSE `summary` for the
previous entry could paint into the new one. The summary-dismiss handler
(outside `performSwap()`) re-checks after its `DELETE`.

### SSE Live Updates

`GET /events` (`handlers/events.rs`) streams per-user events. Mutations call
`EventBus::emit_sidebar` / `emit_summary` (`services/events.rs`, a
`tokio::sync::broadcast` wrapper). `installSse()` in `app.js` handles:

- **`sidebar`** — refetch `/api/sidebar` to update badges.
- **`summary`** — `{entry_id, status}`; updates the row badge and, if open,
  swaps `GET /entries/{id}/summary/fragment` into `#rp-summary-container`.

The stream `select!`s against the global `CancellationToken` for graceful
shutdown. `/events` is registered outside the ETag, Compression, Date-header and
Timeout layers, which would buffer or cut a long-lived stream.

### Caching

- **Sidebar cache** (`sidebar_cache.rs`) — per-user sidebar chrome (tree +
  unread counts), excluding session-specific fields (e.g. masquerade flag).
  Bounded by capacity and TTL; busted by handlers, sync, and the summary worker.
  Each slot has a generation that `bust` bumps (leaving a tombstone);
  `read_chrome_data` captures it before reading and publishes via
  `insert_if_current`, so a bust during population isn't overwritten by a stale
  value. `RDRS_DISABLE_SIDEBAR_CACHE` disables it (E2E only — it seeds SQLite
  directly, bypassing bust hooks).
- **Page cache** (`page_cache.rs`) — helper over `moka::sync::Cache` for
  per-user TTL page payloads.
- **Admin database stats** (`AdminDbStatsCache` in `page_cache.rs`) — the one
  non-per-user cache: site-wide `/statistics` figures (`COUNT(*)` over `entry`
  and `entry_tombstone`, page-count PRAGMAs) in one `()` slot. Never busted; the
  60 s TTL is the whole invalidation strategy.

### Progressive Web App

Installable and offline-tolerant without leaving SSR. By default the browser
stores nothing of the reader's; offline reading is the only opt-in exception,
and turning it off restores that property.

**Manifest.** `static/manifest.webmanifest`, linked from `base.html` with `?v=`,
served under `/static/` (skipped by session, CSRF and forward-auth layers, so no
`Set-Cookie`). Served as `application/manifest+json` (the fallback type would be
rejected under `nosniff`). `start_url` and `scope` are absolute, else scope
would become `/static/`.

**Icons.** `build.rs` renders `icon-192.png`, `icon-512.png`,
`maskable-icon-512.png` from `favicon.svg` into `OUT_DIR`; `static_assets.rs`
embeds them. The maskable icon is 80% scale on opaque `#1A0E08`.

**Service worker.** `static/js/sw.js`, served at `/sw.js` by
`static_assets::service_worker` (root scope sees navigations). Unstamped URL
(a stamp would register a new worker per deploy), so the build version is in the
body and the response is `no-cache`. Registered by `static/js/pwa.js` from
`app_layout.html`, so `/login` and `/setup` never register one.

**Worker caching rules** — an allowlist over its shell cache
(`rdrs-shell-<version>`), since the Cache API ignores `no-store` / `Vary: Cookie`:

- **Navigations** — network-first with navigation preload; fallback to the
  saved library for `/` and `/entries/*`, else the precached `/offline`. Never
  stored.
- **Same-origin `GET /static/`** — cache-first, populated on use; cache is keyed
  by build version and dropped on activate. Disabled for dirty builds via
  `worker_may_cache_static`, derived from `cache_control_for` so the rule lives
  once.
- **Feed icons and proxied images** — network-first, falling back to the
  offline cache; never populated here (that budget is the reader's).
- **Everything else** (`/api/*`, `/events`, fragments) — passthrough, never
  stored.

Precache is just `/offline` and `app.css`; the rest of the shell is only reached
via navigations, which fail offline anyway.

**Offline page.** `GET /offline` (`pages::offline_page`) renders `offline.html`
with no auth and no user data, `public, max-age=3600`. That header stops
`middleware::cache_control` stamping `no-store` and stops
`slide_session_cookie` appending a cookie. `/sw.js` and `/offline` are in all
three middleware skip lists.

### Offline reading

Opt-in via `user_settings.offline_keep` (entry budget; `0` default = store
nothing).

- **Stored markup is the server's.** The client mirrors
  `GET /entries/{id}/fragment` — one renderer, no client templating.
- **Prefetch doesn't mark read.** Mirroring uses `?offline=1`
  (`FragmentQuery::is_prefetch`), which dispatches no write. (`is_speculative_load`
  can't be used: `Sec-Purpose` is forbidden to `fetch()`.)
- **What to hold:** `GET /api/offline/manifest` (`handlers::offline`) returns
  ids, per-entry `updated_at`, the budget, and an opaque `cache_key` — no
  content. The set is `entry::list_offline_set`: newest unread up to budget,
  starred filling the rest.
- **The cache is its own ledger.** `static/js/offline.js` stores panes under
  `/entries/{id}/fragment` with `updated_at` in `x-rdrs-offline-version`; no
  separate index. `updated_at` moves on every write that changes the pane.
  Responses are rebuilt before storage, dropping `Vary: Cookie` (which would
  prevent matches), `Set-Cookie`, and `no-store`.
- Also stores article images (budgeted; constants in `offline.js`) and the
  `/static/` assets saved pages need, including fonts from `app.css`.
- **Who serves reads:** the worker answers navigations and `<img>` loads;
  `performSwap` catches its own failed pane fetch and asks
  `window.rdrsOffline.fragment()`, keeping requests visible to the network
  (and E2E CDP interception).
- **Library page:** `GET /entries/offline` (`pages::offline_entries_page`) lists
  the whole saved set — no Load More, search or bulk actions. Linked by
  `<rdrs-sidebar>` and the scriptless nav whenever `offline_keep > 0` (read from
  `data-offline-keep` beside `data-offline-key`). The worker serves its cached
  copy for dead `/` and `/entries/*` navigations.
- **Per-reader, cleared on sign-out.** Cache name
  `rdrs-offline-<secret::offline_id(user_id)>` (opaque, since page JS sees it).
  `offline.js` deletes every other cache before its first network call using
  `data-offline-key` on `<html>`. The worker drops all `rdrs-offline-*` caches
  on a successful `POST /logout` or `DELETE /api/session`.
- **Offline, only reading works.** `offline.js` marks every server-reaching
  control (all `form`s including GET ones, links the worker can't answer,
  self-submitting selects) with `data-offline-disabled` (`pointer-events:
  none`). Defined as an allowlist of what works (opening saved entries, `/`,
  `/entries/offline`). A `MutationObserver`, active only offline, keeps marks
  current across swaps and sidebar re-renders.
- **Connection lamp.** `<html data-offline>` is `setOffline`'s only output. CSS
  drives the sidebar-header dot from it: muted green online; amber, captioned
  "Offline", breathing (`conn-breathe`, 2.6 s; stopped by
  `prefers-reduced-motion`) while offline. Attribute-driven because the sidebar
  rebuilds `innerHTML` on every mark-as-read. On narrow screens the hamburger
  shows an offline-only badge. No flash banner for connection changes.
- **Nothing is queued.** With `offline.js` present, a failed GET in
  `performSwap` raises a flash and keeps the reader on their list instead of
  navigating away.
- **Offline is detected from failed requests, not `navigator.onLine`** (true
  behind captive portals and under DevTools offline emulation). Inputs: the
  manifest fetch and `window.rdrsOffline.networkFailed()` from `performSwap`.
  While offline, a probe runs every 30 s. An already-open page learns only on its
  first failed request (hence `performSwap` not navigating); a page loaded
  offline learns at boot sync.

### External Services

**Linkding** (`save/linkding.rs`): saves entries as bookmarks; configured per user.

### AI Summarization

Kagi Universal Summarizer integration:
- `summarize/kagi.rs` — API client
- `summary_worker.rs` — background worker
- `summary_cache.rs` — in-memory cache
- `summary_cleanup.rs` — periodic cleanup of orphaned/expired summaries

**Flow:** request → check cache, then DB → otherwise queue to worker → worker
calls Kagi and stores in `entry_summary` → cached and returned.

- **Cancellation:** each queued/in-flight job has a `CancellationToken` in a
  `CancelRegistry` keyed by `(user_id, entry_id)`;
  `POST /entries/{id}/summarize/cancel` cancels it (aborting the HTTP request)
  and deletes the record.
- **Timeout:** 90 s hard timeout per Kagi request; expiry marks it `failed`.
- **Status:** pending, processing, completed, failed (with error message).

## Security

### Signing & the root key (`secret.rs`)

One root key — `RDRS_SECRET`, or random at boot — backs every signature. Each
use has a domain-separation prefix (`image:`, `greader-token:`, `session:`,
`csrf:`, …) so a value for one purpose can't be replayed as another (without
it, the CSRF token would equal the session cookie's signature).

- **Session cookie:** `<session_token>.<hmac>`. Every extractor and
  forward-auth read it via `session_token_from_jar`, verifying before the DB
  lookup; a leaked `session.session_token` is useless without the key.
- **Image-proxy URLs** and the **GReader post token** use their own domains
  (`image_proxy.rs`, `handlers/greader/auth.rs`).

Rotating the key (including restarting without `RDRS_SECRET`) ends all
sessions and breaks image-proxy URLs cached by GReader clients until re-sync.
GReader `ClientLogin` API tokens are matched against the database, not signed.

### CSRF protection

Two independent lines.

- **First line — `tower_http::csrf::CsrfLayer`** (Go 1.25
  `CrossOriginProtection` scheme), stateless, over the whole router.
  - Unsafe methods require `Sec-Fetch-Site` of `same-origin` or `none`;
    `same-site` is rejected (sibling subdomains/ports get the cookie under
    `SameSite=Lax`).
  - Without `Sec-Fetch-Site` (Safari < 16.4): `Origin` authority (host *and
    port*) must byte-match the request authority (request target, else `Host`);
    `Origin: null` is rejected. Scheme is ignored (works behind TLS
    termination), but a proxy that drops the port from `Host` (nginx `$host` on a
    non-default port) breaks it — see README → Production Notes.
  - Neither header (native GReader clients, `curl`, server-to-server; all
    bearer-authenticated) passes.
  - `middleware::csrf::log_cross_site_rejection`, directly outside the layer,
    logs which check fired (`check=sec_fetch_site` / `check=origin_fallback`)
    from the attached `ProtectionError`.
- **Second line — synchronizer token** (`middleware::csrf::csrf_guard`).
  - Token = `secret::derive_csrf` (HMAC of the session token under `csrf:`): no
    column, no query.
  - Unsafe methods must echo it via `X-CSRF-Token` or a `_csrf` urlencoded field
    (body buffered and rebuilt for the handler).
  - Delivered as a readable `csrf_token` cookie; `static/js/csrf.js` adds it to
    same-origin `fetch` and native POST forms. The cookie is never the
    credential; only the derived MAC is.
  - `multipart/form-data` passes through; the OPML-import handler validates it
    itself (also accepts `X-CSRF-Token`).
  - GReader paths are **not** exempt: a browser calling `/reader/api/0/*` with
    an ambient cookie is accepted by `GReaderUser` and waived from the `T` token
    by `verify_post_token_if_needed`. Only `ClientLogin` is skipped
    (`CSRF_SKIP_SUFFIX`) — a forged call needs the attacker's own credentials,
    and login-CSRF is caught by the first line.
  - Requests with **no** session cookie pass (a forged action must ride the
    victim's cookie).
- **Anonymous sessions** (`middleware::csrf::anonymous_session`). Logged-out
  visitors to HTML pages get a signed `session_token` with no `session` row
  (unauthenticated) plus its `csrf_token`, so login/register forms carry a
  token. Layered inside `forward_auth` (a real session's `Set-Cookie` wins);
  skipped for `/api`, `/static`, `/favicon`, `/health`, and GReader prefixes to
  avoid cookie-poisoning shared caches.
- **Keeping cookie names in step.** Browsers may hold both `csrf_token` and
  `__Host-csrf_token` (on `Secure` deployments). Rules:
  - `csrf.js` prefers `__Host-`, mirroring `session_token_from_jar`.
  - `anonymous_session` validates the cookie against
    `derive_csrf(secret, session_token)`, re-mints on mismatch, and expires the
    other name.
  - Both guards `warn!` on rejection (`csrf.cross_site`, `csrf.mismatch` — the
    latter identifies the session only by `secret::audit_id`).
  - Self-healing is required: logout itself is behind `csrf_guard`.

### Response security headers

`middleware::security_headers` has two layers, both **outermost** in
`create_router`, both leaving existing headers alone (reverse proxy wins):

- **`set_security_headers`** — always: `Content-Security-Policy`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy`, `Permissions-Policy`,
  `X-Frame-Options: DENY`, `Cross-Origin-Opener-Policy: same-origin`. Fixed in
  source.
- **`set_hsts`** — `Strict-Transport-Security` only when HTTPS per `Config`
  (`Config::hsts_header_value`).

Outermost because `forward_auth` and both CSRF guards return early on several
paths (session-minting redirect, "not authorized" redirect, 403s). No path skip
list — `/static`, `/health` and the image proxy get them too.

**Strict CSP:**

```
default-src 'self'; script-src 'self'; style-src 'self';
img-src 'self' data:; font-src 'self'; connect-src 'self'; object-src 'none';
base-uri 'self'; form-action 'self'; frame-ancestors 'none'
```

No `'unsafe-inline'`: **no inline `<script>`, `on*=` attributes, `style=`
attributes, or inline `<style>`.** These fail silently in the browser, so a unit
test scans every template *and* `static/js/` file (`innerHTML` and shadow roots
are policed too).

Script replacements:
- Page-specific script → module in `static/js/`, registered in
  `handlers::static_assets::FILES`, loaded via `src`.
- `onsubmit="return confirm(…)"` → `data-confirm="…"` on the `<form>`.
- `onchange="this.form.submit()"` → `data-submit-on-change` on the `<select>`.
- `onerror="this.style.display='none'"` → `data-hide-on-error` on the `<img>`.

Listeners live in `static/js/behaviors.js` (from `app_layout.html`); flash
dismiss is in `components/rdrs-flash.js` (from `base.html`, so `/login` and
`/register` get it). `<script type="application/json">` blocks are unaffected.

Style replacements:
- Static declaration → class in `static/css/app.css` (`.form-inline`, `.text-xs`, …).
- `style="display:none"` toggled by JS → `hidden` attribute / `.hidden` property.
- Per-datum geometry on /statistics → `pct-N` class (0–100 scale at the end of
  `app.css`) chosen by `bar_percent` in `handlers::pages`; `--pct` drives
  `height` in `.stats-bar` and `width` in `.stats-progress-fill`.
- Shadow-DOM styles → constructable stylesheet via `adoptedStyleSheets` (see
  `components/rdrs-kb-help.js`).
- `_icon_sprite.html` → SVG presentation attributes (`width`/`height`/`overflow`),
  so it collapses without a stylesheet.

Setting `element.style` from script is fine — CSP polices markup, not the CSSOM.

**Runtime audit:** `e2e/src/bin/csp_audit.rs` walks the app in Chromium and
fails on any `securitypolicyviolation` (runtime markup, shadow-root `<style>`,
cross-origin `@import`/fonts, unexpected `img-src`). It ends with a **positive
control** (a planted `style=` that must be blocked). CI runs it in the
`e2e-tests` job; locally `cd e2e && cargo run --bin csp-audit`.

Deliberate omissions: **no `Cross-Origin-Resource-Policy`** (would block
absolute `/api/proxy/image` URLs in native GReader webviews); **no
`publickey-credentials-*` in `Permissions-Policy`** (default `self` is what
passkeys need). `Referrer-Policy` is `strict-origin-when-cross-origin`, not
`no-referrer`, because entry-action redirects use the same-origin `Referer`.

### Password Hashing

Argon2id: 19 MiB memory, 2 iterations, parallelism 1.

### Password Policy

`auth::validate_password_strength` gates every *new* password (registration,
change); existing passwords are never re-checked or force-rotated.

1. **Length** 15–128 **characters** (not bytes). 15 is NIST SP800-63B's floor
   without a second factor. Over-long passwords are rejected, never truncated.
2. **zxcvbn** score ≥ 3 (of 0–4), with the username as `user_input`.

No breached-password blocklist: common-password lists are almost entirely
shorter than 15 characters, and zxcvbn catches long structured ones
(`passwordpassword`). Revisit if a second factor lowers the minimum to 8.

zxcvbn costs ~86 µs typically, ~79 ms at 128 characters, so both call sites run
it **behind** the rate limiter. Only the pass/fail gates; the guess count is
never shown.

### Session Management

- Sliding expiry: 7-day TTL, extended when less than half remains; absolute cap
  90 days from creation.
- Token rotation (OWASP "Renewal Timeout") rides the same trigger (~3.5-day
  token life). The extractor requests it (`middleware::auth::RotationSlot`);
  `slide_session_cookie` performs it on the way out, skipping publicly
  cacheable responses (e.g. the feed-icon route) where the new cookie wouldn't
  reach the client. `rotate_token` matches on the old token, so concurrent
  requests can't chain rotations.
- The old token stays valid for `ROTATION_GRACE_SECONDS` (60 s) via
  `session.previous_token`, handled in `find_by_token`; `delete_session` and
  `delete_user_sessions_except` match it too.
- Cookies: `HttpOnly`, `SameSite=Lax`, `Max-Age` = absolute cap. `Secure` from
  `Config::cookie_secure` (`RDRS_PUBLIC_BASE_URL` scheme, overridable via
  `RDRS_COOKIE_SECURE`). All login paths use
  `middleware::auth::build_session_cookie`.
- **Re-authentication** (OWASP "Reauthentication After Risk Events"):
  - `session.last_authenticated_at` records the last credential proof;
    `middleware::auth::RecentlyAuthenticated` requires it within
    `REAUTH_WINDOW_MINUTES` (5). Missing counts as stale.
  - Guards passkey registration (at ceremony *start*, since challenges are
    single-use) and removal — a password change doesn't revoke passkeys.
  - Guards admin actions on other accounts: promote/demote, disable/enable,
    delete, start masquerade (`handlers::admin::require_recent_authentication`).
    Ending a masquerade is **not** guarded (the demanded password would be the
    impersonated account's; it's a de-escalation anyway).
  - `POST /api/session/reauth` reopens the window, sharing the `PasswordChange`
    rate-limit budget. `POST /admin/reauth` is its form twin for the no-JS admin
    panel (inline confirmation form; flash + redirect on refusal). While
    masquerading it verifies the *original* admin's password
    (`session.original_user_id`).
  - Forward-auth sessions are exempt (identity re-asserted each request; JIT
    accounts have unverifiable hashes).
- **Masquerade:** start and stop rotate the session token in the same `UPDATE`
  that swaps `user_id` (privilege change), and reissue **both** session and
  CSRF cookies (CSRF derives from the token).

### Authentication: deliberately not done

Of nine OWASP Authentication Cheat Sheet findings, these were **declined**:

- **Re-hash on login when Argon2 params change.** Belongs with the change that
  raises the params. When written: compare *upward only* and disable under
  `RDRS_FAST_HASH`, or test logins will downgrade hashes.
- **TOTP second factor.** Passkeys and forward-auth already provide strong
  auth; TOTP means owning recovery codes and lost-device flows with no support
  desk. A lighter alternative: a per-user "disable password login once a
  passkey is enrolled" switch.

`PASSWORD_MIN_LENGTH` is 15 *because* there is no second factor; adding one
would allow 8 and make a breached-password blocklist worth revisiting.

### Input Sanitization

- **Third-party credentials** (Linkding, Kagi) are encrypted at rest with
  `XChaCha20-Poly1305` under a key derived from `RDRS_SECRET`
  (`secret::seal`/`secret::open`). Legacy plain JSON stays readable and is
  sealed on next write. A *generated* `RDRS_SECRET` stores plaintext on purpose.
  Undecryptable values are surfaced as such, not as "not configured".
- **Flash cookie** is HMAC-signed (`secret::DOMAIN_FLASH`) by
  `middleware::flash::sign_flash_cookies` (sign on the way out, verify on the
  way in, so handlers need no secret). Payload is base64url so signed bytes
  equal checked bytes. Stops sibling apps/subdomains from injecting banners.
- **Image proxy** serves by **sniffed magic bytes**, not origin
  `Content-Type`; `application/octet-stream` is treated as unlabelled. SVG is
  allowed (`nosniff` + CSP neutralise scripts).
- All HTML sanitized with Ammonia.
- Parameterized SQL throughout, including dynamic filters.
- **SSRF protection** on every fetch of a URL the app didn't choose
  (readability, image proxy, feed discovery, feed sync, icon fetcher, OPML
  import), all via `services::fetch::Fetcher`, which checks:
  - the **URL** before the request (`utils/url_validation`);
  - **every redirect hop** (max 5);
  - **every DNS answer** at connect time (no rebinding window).

  Blocked: loopback, private, link-local, CGNAT (`100.64/10`, Tailscale),
  benchmarking, multicast, reserved, IPv6 ULA/link-local, and IPv4-mapped
  forms. Opt hosts back in with `RDRS_FETCH_ALLOW_PRIVATE_HOSTS`; non-http(s)
  is always refused. Linkding and Kagi use their own clients (user-typed
  addresses).

## Deployment

### Docker

Multi-stage Dockerfile (~50MB, minimal attack surface, cached layers):
1. **chef** - install cargo-chef
2. **planner** - generate dependency recipe
3. **builder** - compile
4. **runtime** - distroless base

### Production Considerations

See [README.md → Production Notes](README.md#production-notes).
