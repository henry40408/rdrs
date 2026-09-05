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
│   ├── forward_auth.rs  # Forward-auth / trusted-header browser login
│   └── request_log.rs   # Per-request duration logging
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

**Asset cache invalidation.** Everything embedded at compile time — CSS, JS,
fonts and the favicons — changes with the build, so anything cached under a
long-lived header must have a URL that changes too. The templates stamp every
reference with `?v={{ git_version }}`, and the handlers only serve
`public, max-age=31536000, immutable` to a request that arrives with that stamp
(`static_assets::cache_control_for`, shared with `handlers/favicon.rs`, which
version-gates it in `cache_control_for_request`). Requests without it — a
browser probing `/favicon.ico`, a crawler, iOS fetching `/apple-touch-icon.png`
with no `<link>` to follow, or an ES-module import written as a bare path — have
no URL left to change on upgrade and get a short TTL instead. Nested JS imports
side-step this by substituting `__RDRS_ASSET_VERSION__` at serve time so they
too request a stamped URL. A `-dirty` build serves `no-cache` throughout, since
the version string does not change between working-tree edits.

### Configuration (`config.rs`)

Loads settings from environment variables:
- `DATABASE_URL` - Database backend: a file path or `sqlite://` URL (SQLite, the zero-config default) or a `postgres://` URL (PostgreSQL); chosen once at startup
- `SERVER_PORT` - HTTP port
- `RDRS_MULTI_USER_ENABLED` - Whether an admin may create accounts beyond the first; there is no self-service sign-up, and `RDRS_SIGNUP_ENABLED` is retired and refuses startup
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
| `user_invite` | One-time links that set an account's password (HMAC-stored) |
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

**Bulk writes report what they changed.** Any action that can touch an unknown
number of rows tells the user how many it actually touched, taking the number
from the database rather than from what the caller asked for — re-marking 40
already-read entries changed nothing, and a flash that says "40" there is
wrong. Form-action endpoints put it in the flash (`mark_read_scoped`, OPML
import via `opml::ImportSummary::describe`, revoke-others, revoke-all-tokens).
The `GReader` endpoints cannot: their body is a bare `OK` that third-party
clients parse literally, so `mark-all-as-read`, `edit-tag` and
`subscription/import` keep that body and carry the count in the
`X-RDRS-Affected` header (`handlers::greader::AFFECTED_HEADER`) instead. rdrs'
own `app.js` reads the header for its flash and falls back to its DOM-row count
only when the header is absent.

### Middleware

- **auth.rs** - Extracts `AuthUser` from session cookie, provides `AdminUser` for admin-only routes
- **flash.rs** - Stores flash messages in cookies for UI feedback

### Account Creation

There is no self-service registration. An anonymous endpoint that accepts a
username inevitably answers whether that username exists, and with no email
channel there is no way to make the answer ambiguous — a caller can simply try
to sign in with the password they just submitted. So the endpoint is gone, and
with it the question.

1. **`POST /api/setup`** creates the *first* account (always an admin) and is
   refused the moment `user::count() > 0` (`Config::can_setup`). With zero
   accounts there is no username to enumerate, which is what makes an anonymous
   endpoint acceptable here and nowhere else. `GET /setup` redirects to
   `/login` once it has been used.
2. **`POST /admin/users`** is how every later account comes into being. The row
   is written with `password_hash = "!"` — an unparseable PHC string, the same
   convention `forward_auth` uses — so no path can sign into it: `verify_password`
   returns false for every input, on the browser form and `GReader`
   `ClientLogin` alike.
3. **`user_invite`** holds a one-time link for that account. The token is
   generated like a session token and stored as an HMAC under
   `secret::DOMAIN_INVITE`, so a database copy alone cannot mint a working
   link, and the raw value exists only in the flash message shown to the admin
   once. Links expire after `INVITE_TTL_DAYS` (7) and re-issuing revokes the
   previous one — two live links for one account would mean revoking the one
   you know about still leaves a way in.
4. **`GET`/`POST /invite/{token}`** is anonymous: the token is the whole
   authority. Every failing case — unknown, expired, already spent — renders
   one identical page that names no account. Redemption spends the invite with
   `UPDATE … WHERE consumed_at IS NULL` *before* writing the password, so two
   submissions racing on one link cannot both succeed with the later silently
   overwriting the earlier's choice. It then clears the account's sessions and
   API tokens, because an admin-issued reset means "whatever came before stops
   working".

The same machinery is rdrs's only password-reset path: issuing a link for an
account that already has a password leaves the old one working until the link
is redeemed, so a reset cannot lock its owner out on its own.

### Authentication Flow

1. User submits credentials to `POST /api/session`
2. Server validates password with Argon2. An unknown username still runs one
   Argon2 verification against a throwaway hash (`auth::verify_dummy_password`)
   before answering, so "no such account" and "wrong password" cost the same —
   the response message is generic, and the clock must be too
3. Creates session record in database
4. Sets the signed session cookie (`<token>.<hmac>`, see [Signing & the root key](#signing--the-root-key-secretrs))
5. Subsequent requests extract user from `AuthUser` extractor, which verifies the signature before the DB lookup

### WebAuthn/Passkey Authentication

RDRS supports passwordless authentication via WebAuthn/Passkey:

**Registration Flow:**
1. User initiates passkey registration from settings
2. Server generates challenge and stores in `webauthn_challenge` table, asking
   for a **discoverable** credential (`residentKey: required`) — sign-in below
   is usernameless, so a credential the authenticator cannot find on its own
   would be registered and then never usable
3. Browser prompts user to create passkey (biometric/security key)
4. Client sends attestation to server
5. Server validates and stores credential in `passkey` table

**Authentication Flow:**
1. User clicks "Login with Passkey"
2. Server generates a discoverable-credential challenge with an **empty
   `allowCredentials`** and reads nothing from the `passkey` table. This is
   load-bearing: filling `allowCredentials` from stored rows (as an earlier
   version did, with every row on the instance) hands each caller a set of
   stable per-user credential IDs, and answering differently for an empty table
   turns the endpoint into an account-existence oracle. The response is now
   identical for every caller and every instance state
3. Browser prompts user to verify passkey
4. Client sends assertion to server
5. Server resolves the credential by its ID, verifies the signature against
   that one stored key (`finish_discoverable_authentication` installs it as the
   allow-list), and creates a session

> **Passkeys enrolled before this change may need re-registering.** They were
> requested as `residentKey: discouraged`, and an authenticator that honoured
> that literally holds a credential the browser cannot discover — with no
> `allowCredentials` to name it, it can no longer be offered at sign-in. Which
> authenticators are affected depends on whether they obey the hint or ignore
> it:
>
> - **Affected**: most security keys, some Windows Hello configurations, and
>   password managers that respect the RP's request — including Bitwarden,
>   whose browser extension records a `discoverable` flag per passkey at
>   creation time and will only answer an empty `allowCredentials` when that
>   flag is true.
> - **Unaffected**: platform passkeys in iCloud Keychain and Google Password
>   Manager, which are stored discoverable regardless of what was asked for.
>
> Nobody is locked out — password sign-in is untouched — but the recovery has
> an ordering trap. `start_registration` puts the account's existing credential
> IDs in `excludeCredentials`, so an authenticator still holding the old
> credential refuses to enrol a second one for the same account
> (`InvalidStateError`, surfaced to the user as a generic failure). The working
> order is: sign in with a password, **delete the old passkey** in
> `/user-settings`, then register a new one.
>
> The failure itself is silent by nature: an authenticator with nothing
> discoverable to offer simply never returns, so the browser times out and
> `login.js` reports "Authentication was cancelled or timed out." There is no
> server-side signal to improve on — the request never reaches rdrs — which is
> why this is written down rather than detected.

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

`/user-settings` lists the current user's non-expired sessions as cards (`.cred-list` / `.cred-card`, shared with the GReader API token list below it), showing the full `user_agent`, `ip_address`, and created/last-active/expires times. The row `id` reaches the template so the revoke-one form can name it; the `session_token` never does. `POST /user-settings/sessions/{id}/revoke` deletes that one session (`session::delete_user_session_by_id`, `user_id`-scoped so a guessed id cannot reach another user's row, and returning the affected-row count so "already gone" is reported as such rather than as success). The caller's own session is refused there — the card renders a "This device" note instead of a button, and the handler re-checks — because signing yourself out is what Sign Out is for. `POST /user-settings/sessions/revoke-others` deletes every one of the user's sessions except the one making the request (`session::delete_user_sessions_except`), letting a user sign out other devices/browsers in one go without ending their own session. It returns the deleted-row count, which the flash reports ("Signed out 2 other sessions.", or an info flash when there was nothing to end) and `audit::sessions_destroyed_bulk` records as its `count` — the audit helper always had the field, and a bulk revocation that cannot say how much it revoked answers neither the user's question nor the auditor's. `POST /user-settings/api-tokens/revoke-all` (`api_token::delete_user_tokens`) works the same way.

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

**What the two timestamps on `/feeds` mean.** `fetched_at` is written on every
attempt, success or failure. `feed_updated_at` is
`effective_feed_updated_at(feed_timestamp, latest_entry_date, http_last_modified)`
— the max of the feed's own `<updated>`/`<lastBuildDate>`/`<pubDate>`, its
newest entry's date, and the response's `Last-Modified`. The three are *maxed,
not ranked*: `.flatten().max()` drops the absent ones, so a feed carrying only a
`Last-Modified` is judged by it instead of being called stale for having no
in-feed date. The ordering that does exist is *inside* each signal, and the two
in-feed ones prefer opposite fields: the feed timestamp is
`updated.or(published)` while an entry's is `published.or(updated)`. An entry
with neither falls back to the feed timestamp, so on such a feed the
"newest entry" signal is a copy of the feed's own date and there are really only
two independent sources. `feed_updated_at` is written through
`COALESCE($5, feed_updated_at)`, so a `304` (or any failure) advances only
`fetched_at` and never clears a date already known. `compute_freshness`
(`handlers/pages/time_format.rs`) grades the result against `FRESH_MAX_DAYS` /
`WARNING_MAX_DAYS`, falling back to `fetched_at` for feeds that publish no dates
at all; the `/feeds` Stale filter matches the stale band only, not the warning
one. Those two constants are passed into `feeds.html` rather than retyped there,
because the page now explains the rule to the user in a `<details>` disclosure
and prose that drifts from the thresholds is worse than no prose. There is no
consecutive-failure counter and no auto-disable: `fetch_error` holds the last
error and is cleared by the next success.

### Entry Retention

**Retention Worker** (`entry_retention.rs`):
- Opt-in per user via `user_settings.retention_read_days` (`0` = disabled); a no-op when nobody has opted in.
- Runs every 24 hours, pruning entries that are read, older than the configured window, and not starred, in batches.
- Each pruned entry records an `entry_tombstone` (`feed_id`, `guid`) so the next feed sync does not re-insert it. Tombstones cascade-delete with their feed.

### Content Processing

**HTML Sanitization** (`sanitize.rs`):
- Uses Ammonia for XSS protection
- Drops `aria-hidden="true"` subtrees before Ammonia runs. Ammonia strips `class`/`style` too, so markup the source site only kept off-screen via its own CSS would otherwise surface as literal text — e.g. the line-number gutter VitePress/Shiki emits beside a code block, which lands as a column of bare numbers under every block. Falls back to the original when that would empty the entry
- Removes tracking parameters (utm_*, fbclid, etc.)
- Blocks tracking domains (pixel.*, analytics.*, etc.)
- Removes 1x1 tracking pixels
- Fixes relative image URLs
- For images lacking `width`/`height`, harvests intrinsic dimensions (from `data-original-width`/`-height` or inline `style`) and injects them so the browser can reserve space
- Tags proxied content images with `data-img-state="loading"`; the reading pane shows a CSS skeleton until the image loads and swaps in a broken-image fallback (with `alt`) on error
- The three pre-Ammonia passes (`aria-hidden`, lazy-image promotion, dimension harvesting) each parse the whole document, and on a real corpus fewer than a quarter of entries need any of them — so each is gated behind an ASCII-case-insensitive substring test and returns a borrowed `Cow` when it cannot possibly match. **A gate must stay a superset of its pass's real trigger**: widening what a pass reacts to without widening its gate makes the pass silently stop firing rather than fail. That is not left to discipline — each gate is a named predicate paired with a `_inner` pass, and `gates_are_supersets_of_the_passes_they_front` asserts over an edge-case corpus that whenever a gate says "skip", running the pass anyway is a no-op. None of the three is a security control; Ammonia's `clean` is ungated and runs on every document, so a gate that got this wrong would cost presentation, never safety

**Full Content Extraction** (`readability.rs`):
- Fetches article URL
- Extracts main content using readability algorithm
- SSRF protection via shared `utils/url_validation` module (blocks private IPs, localhost, internal domains)
- The extraction is stored on `entry.full_content` as **raw** HTML and sanitized
  per render like feed content — `sanitize_html` signs image-proxy URLs with
  `RDRS_SECRET`, so a sanitized copy in the database would outlive its key
- Because it is stored, the reading pane keeps showing it across refreshes, new
  tabs and scriptless page loads; `GET /entries/{id}/fragment?view=original`
  renders what the feed published instead, and dropping the parameter goes back
  without re-fetching. Re-posting the action refreshes the stored copy
- Not mirrored into `content_text`: search covers what the feed published

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

The other image-serving endpoint, `GET /api/feeds/{id}/icon`, caches for the
same day but as `private, max-age=86400`. It sits behind `AuthUser` and is
scoped to the caller's categories, and because it sets its own `Cache-Control`,
`no_store_for_authenticated` steps aside and adds no `Vary: Cookie` — so
`public` would let a shared cache store it under the URL alone and hand one
user's feed icon to another. `private` keeps the browser cache and bars shared
storage. The proxy keeps `public` deliberately: its URLs are signature-bound to
a public image and carry no per-user meaning.

**URL Format:**
- **Relative paths** (default): `/api/proxy/image?url=...&s=...`
  - Used by Web UI (browsers automatically resolve relative paths)
  - Backward compatible behavior when `RDRS_PUBLIC_BASE_URL` is not configured
- **Absolute URLs** (optional): `https://rdrs.example.com/api/proxy/image?url=...&s=...`
  - Used by Google Reader API when `RDRS_PUBLIC_BASE_URL` is configured
  - Required for native RSS clients (e.g., NetNewsWire) that render HTML directly
  - Configured via `RDRS_PUBLIC_BASE_URL` environment variable

### Partial swaps (`data-swap`)

`installSwap()` in `static/js/app.js` intercepts clicks / submits on elements
tagged `data-swap="<selector>"`, fetches the URL, and replaces either the named
target or every `<template data-swap-target="…">` block the response carries.
On a non-2xx response it falls back to a real navigation, so the SSR page stays
the floor. The entries-family routes answer with four shapes:

- **reading pane** — `GET /entries/{id}/fragment` replaces `#reading-pane`
  (plus the row, via multi-target templates) when an entry is opened. The
  pane's Prev/Next targets come from `GET /api/entries/{id}/neighbors`, but
  only when the loaded list cannot answer: rows are flat siblings in the same
  order the endpoint resolves, and none is ever removed client-side, so an
  entry with a row on *both* sides takes its neighbours straight from the DOM.
  The two ends fall through — the first row's list may have been reached with
  an `after` cursor and the last row's may have pages left — as does a scoped
  search, which `NeighborsQuery` has no field for and the server therefore
  answers across the unsearched set.
- **Load More** — `?fragment=1&after=<cursor>` appends rows before `#load-more`.
  How many rows a page holds is the reader's own `entries_per_page`, via
  `entries_page_size` — read per request rather than from the cached chrome, and
  clamped to `MIN..=MAX_ENTRIES_PER_PAGE` because the value reaches a SQL `LIMIT`
  and a `usize` cast. Both the full render and the fragment arm ask for it, so
  the appended page is the same size as the first.
- **list refresh** — `?fragment=1` (no cursor) re-renders `[data-entries-list]`
  and the "Mark N matching" slot, leaving the focused search box alone. That box
  lives in a drawer above `.list-pane-header`, opened by the filter-bar
  magnifier; the server renders it open whenever the request carried `q`, so
  deep links and swaps land in the right state without a client-side flash.
  A search also hides "Mark Above as Read" — under a filtered list its meaning
  collapses into the "Mark N matching as Read" button above it — while the `A`
  shortcut keeps working. The same response backs **Mark Above as Read** and the
  **"Mark as Read..." age dropdown** (both via `refreshEntriesList()`): a bulk
  mark only removes rows, so they re-render the list in place and raise
  `rdrs:sidebar-stale` for the badges instead of reloading the document and
  losing the open entry, the sidebar's loaded feed lists and both scroll
  positions. The inbox serves it too, which is why `/`
  distinguishes `?fragment=1&after=` (Load More) from a bare `?fragment=1`.
- **sidebar navigation** — `?pane=1` (`/categories/{id}/entries` and
  `/feeds/{id}/entries`) replaces the whole `[data-list-pane]` column and resets
  `#reading-pane` to its empty state. `swapListPane()` drives it for sidebar
  category and feed links, the `[` / `]` / `{` / `}` shortcuts and `g c` / `g f`,
  pushes the clean URL with `pushState`, and patches the sidebar's `.active`
  classes *without* re-rendering it — `<rdrs-sidebar>`'s `render()` rebuilds
  `innerHTML`, and a navigation has no reason to pay for that or to churn the
  rows under the pointer. `popstate` reverses it: a category or feed path swaps
  back in place, anything else reloads.

The sidebar lists the **feeds of the open category** underneath it, loaded on
demand from `GET /api/sidebar/categories/{id}/feeds` (with per-feed unread
counts) and mirrored per category in `sessionStorage`. They are deliberately not
part of `/api/sidebar`: that payload is embedded in every logged-in document,
which is `no-store`, so a several-hundred-feed account would pay for its whole
subscription list on every page load to render one category's worth. `?pane=1`
navigation and the `rdrs:sidebar-stale` signal both revalidate it. The `[` / `]`
(and unread-only `{` / `}`) shortcuts walk categories and the open category's
feeds as **one flat list, in the order displayed** — what is on screen is what
the shortcuts step through.

Two per-user settings shape those lists: `user_settings.sidebar_sort` (`name`,
the server's A-Z order, or `unread`, busiest first) and
`user_settings.sidebar_hide_read`, which drops fully-read categories and feeds.
Both ride along in the `/api/sidebar` payload and are applied **client-side**,
in `arrangeSidebarRows()`. The server keeps sending the complete list because
only the client knows which row is active — and the active category or feed
stays listed at zero unread, so finishing the last entry doesn't make the group
the reader is looking at vanish from under the cursor. For the same
don't-move-things-underneath reason, the unread ordering settles on a full
render rather than re-sorting on every badge update; `isStructuralChange()`
treats a changed *visible set* as structural (hidden rows must appear and
disappear promptly) but a changed *order* as not.

`sidebar_hide_read` is what makes that full render an *everyday* path rather than
a navigation one: emptying a group takes it out of the visible set, so an
ordinary mark-as-read is structural. `render()` therefore carries two things
across its own `innerHTML` rebuild — the open category's feed rows, re-adopted by
id so their `<img>` favicons are never recreated, and `.sidebar-nav`'s scroll
offset, since the nav is its own scroller and a reader deep in a long category
list has nothing to do with the entries just triaged.

Swaps that target a reading-pane region (`#reading-pane`,
`#rp-summary-container`) and are *not* pane navigation carry a staleness check:
the entry in the request URL must still be the entry the pane shows when the
response lands, otherwise only the response's flash is applied and the markup is
dropped. Without it an SSE `summary` event for the outgoing entry — which passes
its own `currentPaneEntryId()` pre-check *before* the switch — can paint one
entry's summary into another's pane. The summary-dismiss handler, which clears
the container without going through `performSwap()`, re-checks the same way
after its `DELETE`.

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

### Progressive Web App

The app is installable and survives a lost connection, without giving up the
SSR-first model. By default nothing belonging to a reader is written to disk by
the browser at all; **offline reading is the one opt-in that changes that**, and
the design below is arranged so that switching it off restores the original
property rather than merely leaving it unused.

**Manifest.** `static/manifest.webmanifest`, linked from `base.html` with the
usual `?v=` build stamp and served from under `/static/` — that prefix is
already skipped by the session, CSRF and forward-auth layers, so the response is
guaranteed free of `Set-Cookie` and safe under the `immutable` header. It is
served as `application/manifest+json`; the serve table's fall-through is
`application/javascript`, and with `nosniff` on every response a mislabelled
manifest is rejected outright rather than merely mis-typed. `start_url` and
`scope` are absolute, because a relative value would resolve against the
manifest's own URL and scope the app to `/static/`.

**Icons.** `build.rs` renders `icon-192.png`, `icon-512.png` and
`maskable-icon-512.png` from the same `favicon.svg` as the favicons, into
`OUT_DIR`, and `static_assets.rs` embeds them. The maskable one is drawn at 80%
scale on an opaque `#1A0E08` field so the launcher's crop takes background
rather than artwork.

**Service worker.** `static/js/sw.js`, served from `/sw.js` by
`static_assets::service_worker` — a worker's scope is the directory it was
served from, so only a root-served one sees navigations. Two consequences
follow: the URL carries no `?v=` stamp (a stamped one would register a *new*
worker per deploy instead of updating the existing one), so the build version
travels inside the script body and the response is `no-cache`, not `immutable`.

Registration lives in `static/js/pwa.js`, loaded from `app_layout.html` rather
than `base.html`, so `/login` and `/setup` never register one.

**What the worker does, and what it must never do.** Every signed-in response is
`Cache-Control: no-store` + `Vary: Cookie`, and the Cache API honours neither —
`cache.put` stores whatever it is handed. The rule is therefore enforced in the
worker as an *allowlist* over the shell cache it owns (`rdrs-shell-<version>`),
which cannot drift as routes are added:

- **Navigations** — network-first (with navigation preload, to pay back the
  latency the worker itself adds), falling back to the reader's saved library for
  `/` and `/entries/*` and to the precached `/offline` page otherwise. The
  response is never stored.
- **Same-origin `GET`s under `/static/`** — cache-first, populated on first use.
  Safe because those URLs are cookie-free, version-stamped and `immutable`, and
  the whole cache is keyed by build version and dropped on activate. Turned off
  for a build from a dirty working tree: `git describe --dirty` gives every such
  build the same version string, so the `?v=` URL cannot tell one rebuild from
  the next and the worker would serve a stale stylesheet back. `cache_control_for`
  already drops those responses to `no-cache` for that reason, and
  `worker_may_cache_static` derives the worker's half of the decision *from that
  function* — the Cache API ignores `Cache-Control`, so the worker has to be told
  separately, and two copies of the rule would drift.
- **Feed icons and proxied images** — network-first, falling back to a copy
  already in the reader's offline cache. Never *populated* here: what goes into
  that cache is the reader's budget to spend, and a worker that quietly stored
  every image scrolled past would be spending it behind their back.
- **Everything else** — `/api/*`, `/events`, entry fragments — passes straight
  through and is never written anywhere.

The precache is deliberately just `/offline` and `app.css`: the rest of the app
shell is only reachable from navigations, which fail offline, so precaching it
would buy nothing and cost every visitor the download up front. A reader who
turns offline reading on gets the shell into *their own* cache instead — see
below.

**Offline page.** `GET /offline` (`pages::offline_page`) renders `offline.html`
with no auth extractor and no user data, and answers `public, max-age=3600`.
That header is load-bearing twice: it keeps `middleware::cache_control` from
stamping `no-store` on a request that carries a session, and it is what tells
`slide_session_cookie` the response is publicly cacheable and must not have a
session cookie appended. `/sw.js` and `/offline` are both in all three
middleware skip lists for the same reason.

### Offline reading

Opt-in, bounded, and per-reader. `user_settings.offline_keep` is the number of
entries the browser may hold; `0` is the default and means the browser stores
nothing of the reader's, which is the property the rest of the PWA design
depends on.

**What is stored is the server's own markup.** The client mirrors
`GET /entries/{id}/fragment` responses — the same reading-pane HTML a click
produces — so there is exactly one renderer whether a pane is being displayed or
saved. No client-side templating, no JSON-to-DOM layer, nothing to keep in step
with the Askama templates.

**The prefetch must not consume the queue.** Opening an entry marks it read, and
a sync opens all of them, so the mirroring fetch carries `?offline=1`
(`FragmentQuery::is_prefetch`) which renders the entry exactly as it is and
dispatches no write. The pre-existing `is_speculative_load` check cannot carry
this: it reads `Sec-Purpose`, and `Sec-` headers are forbidden to `fetch()`.

**What to hold** comes from `GET /api/offline/manifest` (`handlers::offline`):
ids, an `updated_at` per entry, the budget, and an opaque `cache_key`. The set
is `entry::list_offline_set` — newest unread first up to the budget, starred
entries filling whatever is left. Deliberately no titles or content: the client
already has the markup, and repeating it would put a second copy of the reader's
data on the wire every sync.

**The cache is its own ledger.** `static/js/offline.js` stores each pane under
the canonical `/entries/{id}/fragment` — the URL a click produces — with its
`updated_at` in an `x-rdrs-offline-version` header. There is no separate index:
the cache keys *are* the list, and `updated_at` moves on exactly the writes that
can change the rendered pane (content, full content, read and star state). Every
response is **rebuilt** before storage rather than put as it arrived, which drops
`Vary: Cookie` (honoured by `cache.match`, while the worker's own `Request`
carries no cookie header — a stored copy could otherwise never match again),
`Set-Cookie`, and the `no-store` the Cache API ignores anyway.

Alongside the panes it stores the article images (budgeted; see the constants in
that file) and the `/static/` assets the saved pages need, gathered off the live
document and out of `app.css` for the fonts. Without the latter the library page
is markup with no stylesheet and no `app.js`, so a click on a row is a real
navigation to a fragment URL that resolves to nothing.

**Reads are split by who can retry.** The service worker answers navigations and
`<img>` loads, because a page cannot rescue either — one replaces the document
before any script runs, the other has no interception point. A reading pane is
neither: `performSwap` issues that fetch itself and, when it throws, asks
`window.rdrsOffline.fragment()` for the saved copy. Keeping it page-originated
means the request stays visible to anything watching the network, the E2E
harness's CDP interception included.

**The library page is the way to all of it.** `GET /entries/offline`
(`pages::offline_entries_page`) lists the whole saved set at once — no Load
More, no search, no bulk actions, because every one of those needs the network.
That is also why it is not optional chrome: an ordinary list is capped at
`entries_per_page` and its Load More reaches the server, so offline a reader
would otherwise see only the first page of what their own browser is holding.
`<rdrs-sidebar>` offers the link, and `app_layout.html`'s scriptless nav a plain
one, whenever `offline_keep > 0` — read off the `data-offline-key`'s companion
`data-offline-keep`, so the sidebar's own payload does not have to carry a value
that only changes across a full page load. The worker answers dead `/` and
`/entries/*` navigations with the cached copy of this page.

**Per-reader, and gone on sign-out.** The cache is named
`rdrs-offline-<secret::offline_id(user_id)>` — an opaque tag rather than a user
id, because the name is handed to page JavaScript. `offline.js` deletes every
cache that is not the current key *before its first network call*, using the
`data-offline-key` the server renders onto `<html>`; signing in as someone else
is only possible online, so that is the first moment after a switch at which the
previous account's articles can go. The worker also intercepts `POST /logout`
and `DELETE /api/session` and drops every `rdrs-offline-*` cache when one
succeeds — the only place all three sign-out paths are observable.

**Offline, only reading works, and the rest says so.** `offline.js` marks every
control that reaches the server — every `form` (including the `method="get"`
ones, Load More and the search box), every link the worker cannot answer, and
the selects that submit themselves — with `data-offline-disabled`, which the
stylesheet gives `pointer-events: none`. The rule is stated as the *allowlist*
of what still works (opening a saved entry; `/` and `/entries/offline`, which
the worker serves) because that list is three items and stable, where a list of
the controls that need the network is dozens and falls behind the next one
added. A `MutationObserver`, running only while offline, keeps the marks true
across swaps and `<rdrs-sidebar>`'s own re-renders.

**The connection is a lamp, not an announcement.** `<html data-offline>` is
`setOffline`'s whole published interface: the stylesheet greys out the controls
above from it, and `<rdrs-sidebar>`'s connection lamp — a dot in the sidebar
header, amber and captioned "Offline" while the attribute is set, muted green
with the word left to screen readers otherwise — is CSS reacting to the same
attribute rather than state the component holds. Offline the dot breathes
(`conn-breathe`, 2.6 s), because offline is a *wait*: `setOffline` re-probes
every 30 s and a solid light reads as a dead indicator where a breathing one
says the app is still trying. Only offline; the light a reader looks at all day
holds still, and the sheet's global `prefers-reduced-motion` rule stops the
breath for anyone who asked it to. That is deliberate: the sidebar
rebuilds its own `innerHTML` on every mark-as-read, and a lamp kept as a
property would need re-applying after each one, with the render that forgot
leaving a green light and no connection. On narrow screens the sidebar is a
closed drawer, so the hamburger carries the offline half of it as a badge —
only that half, since a permanent "all is well" marker on the button is exactly
the noise this replaced. Losing a connection used to raise a flash banner too;
a state that comes and goes on its own belongs in a light that is always there.

Nothing is queued for later. What replaces that is not losing the reader's
place: `performSwap` used to answer a failed GET with a real navigation, so a
dead Load More threw them off the list they still had onto the offline page.
With `offline.js` present it stays put and raises a flash instead.

**Offline is decided by evidence, not by `navigator.onLine`.** That flag reports
having a network interface, not anything answering on it: it stays true behind a
captive portal, and Chrome leaves it true under DevTools' own offline emulation,
so a UI driven by it is disabled in neither case. `setOffline` is instead driven
by requests — the sync's manifest fetch, and `performSwap` reporting a fetch
that threw through `window.rdrsOffline.networkFailed()`. Recovery is a probe
every 30 s while offline, because the only proof the connection is back is a
request that succeeds and the `online` event cannot be relied on to prompt one.

The consequence worth knowing: a page already open when the connection dies
learns of it from its own *first failed request*, not the moment it happens. So
that request must not be the thing that goes wrong — hence `performSwap`
keeping the reader on their list rather than navigating. A page loaded *after*
the fact needs no such luck: `offline.js`'s boot sync fails immediately and
everything is disabled before the reader touches anything.

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
- **Keeping the two sides in step.** The CSRF cookie has two names — `csrf_token`
  and, on a `Secure` deployment, `__Host-csrf_token` — and a browser can hold
  both at once (one minted before the upgrade that introduced the prefix, or
  before `RDRS_COOKIE_SECURE` was flipped). Three rules stop the front end and
  the back end picking different ones, which would 403 every unsafe request:
  `static/js/csrf.js` prefers the `__Host-` name, mirroring
  `session_token_from_jar`; `anonymous_session` *validates* the cookie against
  `derive_csrf(secret, session_token)` rather than merely checking it is present,
  re-minting on any mismatch and expiring the leftover under the other name (safe
  because the cookie is never the credential); and both guards `warn!` on
  rejection (`csrf.cross_site`, `csrf.mismatch`, the latter identifying the
  session only by `secret::audit_id`) so a bodyless 403 is diagnosable from the
  log. Self-healing matters more than it looks: logout is itself behind
  `csrf_guard`, so a diverged browser has no in-app escape hatch.

### Response security headers

`middleware::security_headers` ships two layers, both applied **outermost** in
`create_router` and both leaving a header the response already carries
untouched (so a reverse proxy's value wins):

- **`set_security_headers`** — unconditional. `Content-Security-Policy`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy`, `Permissions-Policy`,
  `X-Frame-Options: DENY`, `Cross-Origin-Opener-Policy: same-origin`. Values are
  fixed in the source; nothing is configurable.
- **`set_hsts`** — `Strict-Transport-Security`, and only when `Config` says the
  deployment is HTTPS. See `Config::hsts_header_value`.

Outermost placement is load-bearing rather than stylistic: `forward_auth` and
both CSRF guards return a response *without* calling `next` on several paths (the
forward-auth redirect that mints the session cookie, its "not authorized"
redirect, and the guards' 403 rejections), so a layer nested inside them would
never see those responses. There is likewise no path skip list — `/static`,
`/health` and the image proxy all carry the headers, and `nosniff` on a proxied
image is exactly where it earns its keep.

**The CSP is strict on both scripts and styles**, which constrains how the
frontend may be written:

```
default-src 'self'; script-src 'self'; style-src 'self';
img-src 'self' data:; font-src 'self'; connect-src 'self'; object-src 'none';
base-uri 'self'; form-action 'self'; frame-ancestors 'none'
```

Neither directive carries `'unsafe-inline'`, so **no markup may ship an inline
`<script>`, an `on*=` handler attribute, a `style=` attribute or an inline
`<style>` element.** None of those is a build error — they simply stop working
in the browser — so a unit test walks every template *and* every file under
`static/js/` and fails on any of them. The JS half matters because markup a
script assigns to `innerHTML` is parsed and policed exactly like markup from the
server, shadow roots included.

Script replacements:

- Page-specific script → a module under `static/js/`, registered in
  `handlers::static_assets::FILES` and referenced with `src`.
- `onsubmit="return confirm(…)"` → `data-confirm="…"` on the `<form>`.
- `onchange="this.form.submit()"` → `data-submit-on-change` on the `<select>`.
- `onerror="this.style.display='none'"` → `data-hide-on-error` on the `<img>`.

The delegated listeners behind those `data-` attributes live in
`static/js/behaviors.js` (loaded from `app_layout.html`); the flash banner's
dismiss button is handled in `components/rdrs-flash.js`, which `base.html` loads
so `/login` and `/register` get it too. `<script type="application/json">`
bootstrap blocks are unaffected — the browser never executes them, so CSP does
not police them.

Style replacements:

- A static declaration → a class in `static/css/app.css` (`.form-inline`,
  `.text-xs`, …).
- `style="display:none"` on something JS later reveals → the `hidden` attribute,
  toggled through the `.hidden` property.
- Per-datum geometry on /statistics → a `pct-N` class off the 0–100 scale at the
  end of `app.css`, picked by `bar_percent` in `handlers::pages`. CSS cannot read
  a number out of an attribute, so the server rounds to a whole percent and
  selects from a finite set. `--pct` is read as `height` by `.stats-bar` and as
  `width` by `.stats-progress-fill`, so one scale drives both charts.
- Shadow-DOM component styles → a constructable stylesheet adopted via
  `adoptedStyleSheets`, as `components/rdrs-kb-help.js` does.
- `_icon_sprite.html` → SVG presentation attributes (`width`/`height`/
  `overflow`), which are not `style` attributes and so are not policed. It needs
  to collapse even with no stylesheet, since a bare `<svg>` renders at 300×150.

Writing to `element.style` **from script** stays legal throughout — CSP polices
markup, not the CSSOM.

The static scan is only half the guard. `e2e/src/bin/csp_audit.rs` walks the
app in Chromium, listens for `securitypolicyviolation`, and fails on anything the
browser blocks — covering what a source grep cannot see: markup injected at
runtime, `<style>` inside a shadow root, a cross-origin `@import` or webfont, an
`img-src` the templates never mention. It ends with a **positive control** — a
planted `style=` that the policy must reject — so a clean report can only mean
the collector was live, never that it silently stopped observing. CI runs it as
one step of the `e2e-tests` job, reusing that job's build; locally it is
`cd e2e && cargo run --bin csp-audit`.

Two omissions are deliberate and documented in the module: **no
`Cross-Origin-Resource-Policy`** (it would block the absolute `/api/proxy/image`
URLs the GReader item feed hands to native clients rendering in a webview), and
**no `publickey-credentials-*` entry in `Permissions-Policy`** (unlisted features
keep their `self` default, which is what passkeys need). `Referrer-Policy` is
`strict-origin-when-cross-origin` rather than `no-referrer` because the
entry-action redirect recovers the originating list from the same-origin
`Referer`.

### Password Hashing

Uses Argon2id with:
- Memory: 19 MiB
- Iterations: 2
- Parallelism: 1

### Password Policy

`auth::validate_password_strength` is the single gate for every *new*
credential (registration and change-password; existing passwords are never
re-measured and never force-rotated). It checks, in order:

1. **Length**, 15–128 **characters** — not bytes, so the rule means the same
   thing in every script. 15 is NIST SP800-63B's floor for an account with no
   second factor, which is what a password-protected rdrs account is. Over-long
   passwords are rejected, never truncated.
2. **Guessability**, via zxcvbn, refusing anything scoring below 3 on its 0–4
   scale. The username is passed in as a `user_input`, so a password built out
   of the account name is scored for what it is.

There is deliberately **no breached-password blocklist**. The cheat sheet
offers both controls, but the length minimum already does the blocklist's job:
common-password corpora are overwhelmingly short (in SecLists' 10k-most-common
list exactly one entry reaches 15 characters), so a list consulted after the
length check would catch almost nothing while adding itself to the binary. What
survives 15 characters is *structure* — `passwordpassword`, `qwertyuiopasdfgh`,
`aaaaaaaaaaaaaaaa` — which is what the estimator is built to score. If a second
factor ever lands and the minimum drops to 8, that calculus reverses and a
blocklist becomes worth revisiting.

Cost matters for ordering: zxcvbn runs in ~86µs on a typical password but
~79ms on a 128-character worst case (measured in release), which is Argon2
territory. Both call sites therefore run it **behind** the rate limiter, for
the same reason hashing does — otherwise the caller, not the server, chooses
how much CPU a rejected attempt costs. The score gates the request; the guess
count is never shown, since advertising an entropy figure as a strength
guarantee is exactly what the cheat sheet warns against.

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
  picked-up session must not be able to add one silently. The same window
  guards every admin action that changes *another* account — promote/demote,
  disable/enable, delete, and starting a masquerade
  (`handlers::admin::require_recent_authentication`). Ending a masquerade is
  deliberately **not** guarded: while masquerading, the password that would be
  demanded belongs to the impersonated account, so the check could never be
  satisfied and the admin would be stranded inside the impersonation — and
  stepping back down is a de-escalation anyway.
  `POST /api/session/reauth` re-opens the window against the account password
  and shares the `PasswordChange` rate-limit budget, so it cannot be used to
  sidestep that limit. `POST /admin/reauth` is its form-encoded twin: the admin
  panel is server-rendered with no JavaScript to catch a 403 and re-prompt the
  way `passkey.js` does, so `/admin` renders an inline confirmation form
  whenever the window has lapsed and the refusal is an ordinary flash +
  redirect. While masquerading, that endpoint verifies the *original* admin's
  password (`session.original_user_id`), not the impersonated account's. The
  check sits on the *start* of the registration
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

### Authentication: deliberately not done

A review against OWASP's Authentication Cheat Sheet produced nine findings.
Eight are implemented and documented above. The remaining two were considered
and **declined**, which is recorded here so the next reader does not spend the
evaluation again — and so that reopening either is a decision rather than an
oversight.

**Re-hashing a password on login when the Argon2 parameters change.** The
Password Storage Cheat Sheet asks for it, and it is perhaps thirty lines. It
buys nothing until someone actually raises the parameters, so it belongs in
that change rather than ahead of it. Whoever writes it should note the trap:
the comparison has to be *upward only* and disabled under `RDRS_FAST_HASH`, or
every test-suite login will quietly downgrade a strong hash to the minimal-cost
parameters that flag selects.

**A real second factor (TOTP).** rdrs already offers two routes to strong
authentication — passkeys, which are phishing-resistant and, since the
discoverable-credential flow, usable without a username; and forward-auth,
which hands the whole question to an IdP built for it. Implementing TOTP here
would mean owning recovery codes and the lost-device path, which is where these
features actually fail, and a self-hosted reader has no support desk to catch
whoever gets stranded. If an in-tree option is ever wanted, the small version
is a per-user "disable password login once a passkey is enrolled" switch: it
gets phishing-resistant single-factor sign-in with none of the recovery
machinery.

Note that the second decision holds the first password-policy constant in
place: `PASSWORD_MIN_LENGTH` is 15 *because* there is no second factor. A real
one would let it drop to 8 (NIST SP800-63B), and would also make a breached-
password blocklist worth revisiting — see the note under Password Policy on why
one is not there today.

### Input Sanitization

- All HTML content sanitized with Ammonia
- SQL injection prevented via parameterized queries throughout (including dynamic filter conditions)
- SSRF protection on every outbound fetch of a URL the app did not choose:
  readability, the image proxy, feed discovery, feed sync, the icon fetcher and
  OPML import all go through `utils/url_validation`. The image proxy and
  readability apply `validate_url` directly; the feed-side paths take a
  `FetchPolicy`, which is `validate_url` plus the hosts a deployment opted back
  in to via `RDRS_FETCH_ALLOW_PRIVATE_HOSTS` (a LAN feed is ordinary use for a
  self-hosted reader). A non-http(s) scheme is refused whatever the allow list
  says. The image proxy's `If-None-Match` 304 is answered *after* signature
  verification: ahead of it, an unsigned request could mint a cacheable 304 that
  echoed its own `s` back as the `ETag`.
  Redirects and DNS resolution are **not** yet covered — `SHARED_CLIENT` still
  follows redirects with no per-hop check, and a hostname is validated as a
  string rather than by its resolved addresses

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
