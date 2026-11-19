# Architecture

RDRS is a self-hosted RSS reader built with Go. It ships as one static binary
(CGO off) with every CSS, JS, font and icon file embedded.

## Overview

```
┌─────────────────────────────────────────────────────┐
│        Views (internal/views, string markup)        │
├─────────────────────────────────────────────────────┤
│       HTTP Layer (net/http handlers, middleware)    │
├─────────────────────────────────────────────────────┤
│     Services (feedsync, worker, summarize, ...)     │
├─────────────────────────────────────────────────────┤
│              Models (SQL per table)                 │
├─────────────────────────────────────────────────────┤
│           Database (SQLite / PostgreSQL)            │
└─────────────────────────────────────────────────────┘
```

## Directory Structure

The module is `github.com/henry40408/rdrs`. Dependencies are vendored.

```
cmd/rdrs/            # Entry point: flags, logging setup, wiring, graceful shutdown
internal/
├── audit/           # Session/credential audit events (log target `rdrs::audit`)
├── auth/            # Session/CSRF cookies, sliding sessions, passwords (Argon2id),
│                    #   rate limiter, client IP, forward-auth login, WebAuthn setup
├── cache/           # In-memory caches: TTL map, sidebar chrome, summary state, admin DB stats
├── config/          # Environment configuration, validation, startup warnings
├── datetime/        # Date parsing (RFC 2822, ISO 8601, Chinese dates) and display
├── db/              # Pool per backend, dialect forks, SQLite write-priority scheduler, maintenance
├── discovery/       # Feed URL detection
├── events/          # In-memory event bus for SSE live updates
├── feedparse/       # RSS / Atom / RDF / JSON Feed parser, charset decoding
├── feedsync/        # Feed refresh: conditional fetch, parse, entry upsert, bucket refresh
├── fetch/           # SSRF-guarded HTTP client for untrusted URLs, retry, timeouts
├── flash/           # One-shot banner messages in a signed cookie
├── greader/         # Google Reader API: stream/item ids, JSON shapes, POST token
├── han/             # Simplified-Chinese detection for the `lang` attribute
├── htmldepth/       # Workaround for x/net/html's 511-open-element limit
├── icon/            # Feed icon fetching
├── imageproxy/      # Signing and verification of image-proxy URLs
├── migrate/         # Embedded SQL migration runner
├── models/          # SQL for every table (entry/ holds the entry queries)
├── opml/            # OPML import/export
├── pixel/           # Open-tracking pixel injection
├── readability/     # Full-content extraction
├── sanitize/        # HTML sanitization and tracking removal
├── save/            # External save targets (Linkding)
├── search/          # Boolean query parser for /search
├── secret/          # Keyed derivation from RDRS_SECRET (domain-separated), sealing
├── server/          # Router, handlers, page/fragment/JSON/GReader endpoints
│   └── middleware/  # Cross-site guard, CSRF, session sliding, forward-auth, headers, ETag, ...
├── siphash/         # SipHash variants kept for stored bucket and entry ids
├── summarize/       # Kagi summarizer client, job queue and worker
├── testutil/        # Shared test fixtures
├── textutil/        # HTML to plain text, entity decoding
├── timezone/        # Per-user IANA timezone: local dates <-> UTC instants
├── version/         # Build version
├── views/           # Page markup (layout, macros, one file per area)
├── worker/          # Background loops: sync, cleanup, retention, content_text backfill
└── zxcvbn/          # Password strength estimator (fork of trustelem/zxcvbn)

migrations/          # Embedded SQL: sqlite/ and postgres/
static/              # CSS, JS (vanilla ES modules), fonts, icons, web manifest (go:embed)
e2e/                 # Separate Go module: godog + chromedp suite and audits
```

## Core Components

### Application Entry (`cmd/rdrs`)

- `main.go` parses options, refuses retired environment variables, loads and
  validates `config`, then starts the server and blocks until SIGINT/SIGTERM.
- `app.go` opens the database, runs migrations, builds `server.App` and the
  background workers (`worker.Sync`, the summary worker, `worker.Cleanup`,
  `worker.Retention`, `worker.BackfillContentText`), each on a `db.Background()`
  handle where it writes.
- **Shutdown** runs in two stages: first the HTTP server stops accepting
  requests (open requests get 10 s; SSE streams end at once), then the workers
  are joined (30 s) and the database closes.
- The one-time **setup token** is printed to stderr, not through the logger,
  and only while no account exists.

### Router (`internal/server/routes.go`)

`App.routes()` is the single registry of `Route{Method, Path, Handler}`;
`internal/server/testdata/routes.txt` pins the table. Patterns are
`net/http.ServeMux` patterns with the method included, so a known path with
another method answers 405 with `Allow`. Unmatched paths render the 404 page
when signed in and redirect to `/login` otherwise.

**Middleware stack**, outermost first (`buildHandler`):

1. `RequestLog` (one event per request, labelled by route template, never the
   path; `http.slow_request` at WARN past 1 s)
2. `HSTS` (only when HTTPS per `Config`)
3. `SecurityHeaders`
4. the core stack: `CrossSiteGuard`, `SignFlashCookies`, `ForwardAuth`,
   `SlideSession`, `AnonymousSession`, `CSRFGuard`, `Timeout` (120 s to the
   response head, then 408), `Compress` (gzip/br), `DateHeader`, `ETag`,
   `NoStoreForAuthenticated`

`/events` (SSE) is mounted outside the core stack: Timeout, Compress and ETag
would cut or buffer a long-lived stream.

**Asset cache invalidation.**
- Templates stamp every embedded asset URL with `?v=<version>`.
- Only stamped requests get `public, max-age=31536000, immutable`
  (`server/static.go`; favicons too). Unstamped requests (`/favicon.ico`
  probes, `/apple-touch-icon.png`) get a short TTL.
- Nested JS imports get the stamp by substituting `__RDRS_ASSET_VERSION__` at
  serve time.
- A `-dirty` build serves `no-cache` throughout (its version string does not
  change between edits).

### Configuration (`internal/config`)

Loads settings from environment variables (full table in
[README.md → Configuration](README.md#configuration)):
- `DATABASE_URL` - file path / `sqlite://` (SQLite, default) or `postgres://` / `postgresql://`; chosen once at startup
- `RDRS_SERVER_BIND` - listen address (default `127.0.0.1:8080`)
- `RDRS_MULTI_USER_ENABLED` - whether an admin may create accounts beyond the first; no self-service sign-up (`RDRS_SIGNUP_ENABLED` is retired and refuses startup)
- `RDRS_SECRET` - root HMAC key for every signature, domain-separated in `internal/secret`; a missing or too-short (< 16 bytes) value is replaced by a random per-process key and a warning is logged
- `RDRS_AUTH_PROXY_HEADER` - forward-auth username header; empty disables
- `RDRS_TRUSTED_PROXY_NETWORKS` - CIDRs/IPs whose TCP peer may supply the identity header; required with `RDRS_AUTH_PROXY_HEADER`
- `RDRS_AUTH_PROXY_USER_CREATION` - JIT-create unknown proxy users (default `false`; otherwise redirect to `/login`)
- `RDRS_AUTH_PROXY_GROUPS_HEADER` - comma-separated groups header
- `RDRS_AUTH_PROXY_ADMIN_GROUP` - group granting admin; active only with the groups header
- `RDRS_DISABLE_LOCAL_AUTH` - hides the password form, `POST /api/session` → 403; GReader `ClientLogin` and passkeys unaffected; refuses startup without `RDRS_AUTH_PROXY_HEADER`
- `RDRS_AUTH_PROXY_LOGOUT_URL` - Sign Out redirects here to end the IdP session; if unset, forward-auth Sign Out clears the local session and warns the user to log out at the proxy
- `RDRS_LOG_LEVEL` / `RDRS_LOG_FORMAT` (or `--log-format`) - see [Logging](#logging)

**Retired variables.** `ensureNoLegacyEnvVars` (`cmd/rdrs/env.go`) refuses to
start while an old name is set (`SERVER_BIND`, `IMAGE_PROXY_SECRET`,
`SIGNUP_ENABLED`, `RUST_LOG`, ...), naming the replacement: a silently ignored
variable (say, falling back to a random secret) is worse than a startup
failure. Update that list whenever a variable is added, renamed or removed.

### Logging

`log/slog` with the standard text and JSON handlers (`cmd/rdrs/logging.go`).
- Level: `RDRS_LOG_LEVEL` = `debug` | `info` (default) | `warn` | `error`. There
  is no per-module filter. `RUST_LOG` is retired; the binary refuses to start
  while it is set.
- Format: `--log-format` or `RDRS_LOG_FORMAT` = `full` (default) | `compact` |
  `pretty` | `json`. `json` prints slog JSON lines (`time`, `level`, `msg`, then
  attributes); the other three are aliases of slog's `key=value` text format.
- Calls use a static message and an `event` attribute (`csrf.cross_site`,
  `sync.bucket_started`, ...), with values as typed fields.
- Only the audit logger sets a `target` attribute (`rdrs::audit`).

### Error Handling (`internal/server/errors.go`)

`AppError` carries a status and a client-visible message; the JSON body is
always `{"error": message}`. Auth → 401, forbidden → 403, not found → 404,
validation → 400, rate limited → 429, upstream fetch failure → 502, internal
and database errors → 500 with the cause hidden from the client. Malformed
requests (bad JSON, missing form field) get plain-text rejections.

## Data Layer

### Schema & Migrations (`migrations/`)

Embedded per backend (`migrations/sqlite/`, `migrations/postgres/`) and applied
at startup by `internal/migrate`. Schemas are equivalent; dialect differences
(`BIGINT` for non-id integers, `GENERATED ALWAYS AS IDENTITY`) live in the
Postgres migrations. The runner keeps the `_sqlx_migrations` bookkeeping table
byte-compatible with sqlx (checksums, PostgreSQL advisory lock), so a database
written by the earlier Rust releases keeps working: never reformat or edit an
applied migration.

| Table | Purpose |
|-------|---------|
| `user` | User accounts with role (admin/user) |
| `session` | Session tokens: masquerade support, `previous_token`/`previous_token_expires_at` rotation grace, `last_authenticated_at` for re-auth, `user_agent`/`ip_address`/`last_seen_at` |
| `api_token` | GReader `ClientLogin` tokens, separate from sessions |
| `category` | Feed categories per user |
| `feed` | Feed metadata with etag caching and bucket assignment |
| `entry` | Feed items with read/starred status |
| `entry_open` | Rendered-entry opens (open-tracking pixel) |
| `entry_summary` | AI-generated article summaries |
| `entry_tombstone` | Retention-pruned entries; blocks re-insertion on sync (cascades with feed) |
| `image` | Polymorphic image storage |
| `user_settings` | User preferences and service configs |
| `passkey` | WebAuthn credential storage |
| `user_invite` | One-time links that set an account's password (HMAC-stored) |
| `webauthn_challenge` | WebAuthn challenge state |

### Database Handle (`internal/db`)

- `db.DB` wraps one `database/sql` pool and a `Dialect`, selected by
  `DATABASE_URL`: `modernc.org/sqlite` (pure Go) or `pgx` (via its `stdlib`
  adapter). `db.Executor` is what models accept, so one function runs on a
  `DB` or inside a `Tx`.
- Queries are written once with `$N` placeholders. Dialect forks go through
  `Dialect` methods (`LikeCI`, `CursorTS`, `DaysAgo`, `IndexedBy`,
  `NullSafeNe`, ...) and `Dialect.Rewrite` (`datetime('now')` → `now()`).
- The entry upsert's NULL-safe inequality is a deliberate hand-dispatched pair
  (`NullSafeNe`: `IS NOT` / `IS DISTINCT FROM`) because a blanket `IS NOT`
  rewrite would also rewrite every `IS NOT NULL`.
- SQLite: WAL, `synchronous=NORMAL`, `busy_timeout=5000`, `foreign_keys=1`,
  `analysis_limit=400`, at most 5 connections, and every transaction is
  `BEGIN IMMEDIATE` (a deferred read-to-write promotion returns `SQLITE_BUSY`
  without honouring the timeout). PostgreSQL: at most 10 connections, session
  `timezone=UTC` so timestamp-string cursors match SQLite.
- Instants are UTC everywhere and the browser localises what it shows. The one
  place a calendar day matters is `/statistics`: it cuts days in the user's
  `user_settings.timezone` (default `UTC`, `internal/timezone`). Range bounds
  are the UTC instant of a local midnight, and the daily-read chart buckets in
  Go (SQLite cannot apply a zone), so both backends share one path.
- Feed dates without an offset are read as UTC (`internal/datetime`), never in
  the server's `TZ`.

**Planner statistics (SQLite only).** SQLite never refreshes `sqlite_stat1`
itself, and stale (or missing, for a new index) stats cost plans.
`DB.Optimize()` (`PRAGMA optimize`) runs after migrations and on every
retention tick after the first. No-op on PostgreSQL.

**Write-priority scheduling (SQLite only).** SQLite has one writer under WAL.
A `DB` carries a `Priority` (`User` by default; background workers use
`db.Background()`) and a shared scheduler whose admission gate holds a
background operation until no `User` operation is in flight. Rows and
transactions hold admission until closed, so always close them. No-op on
PostgreSQL.

### Models (`internal/models`)

Each file holds the SQL for one table: a struct matching the schema, plain
functions taking a `db.Executor`, and parameter structs instead of long
argument lists. Entry queries live in `models/entry` (filters, cursor
pagination, neighbours, marks, upsert, retention, offline set).

Global `/search` supports a boolean query language (`is:`, `feed:`,
`category:`, `title:`, `author:`, `before:`, `after:`, `AND`/`OR`/`NOT`,
grouping, quoting, `-` negation). `internal/search` parses it into a node tree
and `models/entry/filters.go` renders it per `Dialect` using
case-insensitive `LIKE` (no full-text index). Scoped per-view search uses a
plain substring filter.

## HTTP Layer

### Handlers

Handlers live in `internal/server`, one file per area (auth, account, admin,
categories, feeds, entries, search, statistics, summarizer, media, passkey,
invite, events, GReader). The `Authenticator` in `internal/auth` turns the
session cookie into a `Principal` (`Authenticate` for API routes, where a
store failure is an error; `AuthenticatePage` for pages, which redirect to
`/login`).

**Bulk writes report what they changed**, using the database's affected-row
count, not the requested count.
- Form actions put it in the flash (mark-read, OPML import via
  `opml.ImportSummary.Describe`, revoke-others, revoke-all-tokens).
- GReader `edit-tag` and friends keep the bare `OK` body, so the count goes in
  the `X-Rdrs-Affected` header.

**Bulk mark-as-read is scoped by the rendered list.** The "Mark as Read..."
dropdown sends the list's `data-snapshot-at` as `snapshot` (an rdrs extension
to `mark-all-as-read`; other GReader clients omit it). Entries with
`created_at` at or after it stay unread, so a feed fetch between render and
click cannot mark what the reader never saw. A malformed snapshot is refused,
never treated as "no limit". `older_than_days` alone cannot do this: it tests
`published_at`, which a late arrival may already have in the past.

**Today** (`/entries/today`, sidebar item after Unread) is the unread entries
whose `COALESCE(published_at, created_at)` falls on the current day in the
user's timezone. `entry.TodayUnreadFilter` is the one definition behind the
list, the sidebar badge, the reader's neighbours and mark-all, so the badge
always equals the list. Day bounds come from `timezone.StartOfDayUTC` (23 or 25
hours across DST). Its "Mark all as Read" posts to
`/entries/today/mark-read` with the same `snapshot` rule.

### Middleware (`internal/server/middleware`)

See [the stack above](#router-internalserverroutesgo). Session handling lives
in `internal/auth` (`Authenticator`, `Cookies`); the layers only apply it.

### Browser-only facts

`PUT /api/user/client-info` takes what only the browser knows (today its
timezone). It stores nothing: it compares each fact with the account's settings
and answers `200 {"hints": [...]}`; the page shows each hint with
`<rdrs-flash>.show()`. A hint links to the settings form
(`/user-settings?timezone=…` pre-fills the field, marked unsaved; the user
still presses Save). Hints are not flash cookies: the report is sent after the
page has loaded, and a cookie set then is consumed by whichever request reads
flashes next, often a fragment swap that renders none.

`static/js/client-info.js` sends a report only when it differs from the last
one this browser delivered (`localStorage`, keyed per account), which is what
makes an offer one-time per browser. Zones are compared by behaviour over the
next year (`timezone.SameRules`), since browsers report aliases such as
`Asia/Calcutta`.

### Account Creation

No self-service registration: an anonymous endpoint taking a username always
leaks whether it exists.

1. **`POST /api/setup`** creates the first account (admin), refused once an
   account exists (`Config.CanSetup`). `GET /setup` then redirects to
   `/login`. It also requires the setup token (random per start, printed on
   stderr only while setup is open), compared in constant time
   (`secret.VerifySetupToken`), so an anonymous bot cannot claim a fresh
   public deploy. An empty token never means "no token": setup is refused. The
   count, the user insert and the default category share one transaction
   (`Tx.LockUserTable` serializes concurrent setups on PostgreSQL).
2. **`POST /admin/users`** creates every later account with
   `password_hash = "!"` (unparseable PHC, same as forward-auth), so
   `VerifyPassword` fails for all input, browser and GReader alike.
3. **`user_invite`** holds a one-time link. Token generated like a session
   token, stored as an HMAC under `secret.DomainInvite`; raw value shown once
   in the admin's flash. Expires after `InviteTTL` (7 days); re-issuing revokes
   the previous link.
4. **`GET`/`POST /invite/{token}`** is anonymous. Every failure (unknown,
   expired, spent) renders one identical page naming no account. Redemption
   spends the invite with `UPDATE … WHERE consumed_at IS NULL` (so racing
   submissions can't both win), then sets the password and clears the
   account's sessions and API tokens.

This is also the only password-reset path; the old password keeps working until
the link is redeemed.

### Authentication Flow

1. Credentials posted to `POST /api/session` (JSON) or `POST /login` (form)
2. The per-IP and per-account rate limits are charged first, then Argon2
   verification. Unknown usernames still verify against a throwaway hash
   (`auth.VerifyDummyPassword`) so timing doesn't reveal account existence;
   the error message is generic
3. Session row created
4. Signed session cookie set (`<token>.<hmac>`, see [Signing & the root key](#signing--the-root-key-internalsecret))
5. On later requests the signature is verified before the DB lookup

**Rate limiting** (`auth.RateLimiter`): fixed-window counters in a fixed
16 384-slot table (constant memory; keys hash with a per-process seed), in
independent buckets (`login`, `account setup`, `passkey probe`,
`password change`) so abuse of one endpoint cannot lock users out of another.
Defaults: 5 attempts per 60 s per client IP (`RDRS_LOGIN_RATE_LIMIT_ATTEMPTS`,
`RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS`; `0` attempts disables), and a deliberately
wider per-account budget (4x), since anyone can aim an account budget at a
victim. A successful credential check refunds the attempt. Limits run before
any query or Argon2 work.

### WebAuthn/Passkey Authentication

Uses `go-webauthn`. The relying party comes from `RDRS_WEBAUTHN_RP_*`; an
invalid RP fails startup rather than every ceremony later.

**Registration:**
1. User starts registration from settings (requires recent authentication)
2. Server stores a challenge in `webauthn_challenge`, requiring a
   **discoverable** credential (`residentKey: required`) and user verification
   (only ES256 and RS256 offered), since sign-in is usernameless
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
5. Server resolves the credential by ID, verifies against that key, and
   creates a session

> **Older passkeys may need re-registering.** They were created with
> `residentKey: discouraged`; authenticators that honoured it hold
> non-discoverable credentials that can't be offered without `allowCredentials`.
>
> - **Affected**: most security keys, some Windows Hello setups, password
>   managers that respect the hint (e.g. Bitwarden, which only answers an empty
>   `allowCredentials` for passkeys flagged `discoverable`).
> - **Unaffected**: iCloud Keychain, Google Password Manager.
>
> Recovery order matters: registration lists existing credential IDs in
> `excludeCredentials`, so the authenticator refuses a second one
> (`InvalidStateError`). Sign in with a password, **delete the old passkey** in
> `/user-settings`, then register. The failure is a client-side timeout
> ("Authentication was cancelled or timed out." in `login.js`) that never
> reaches the server, so it can't be detected.

### Forward-Auth (Trusted-Header) Login

The `ForwardAuth` middleware (with `auth.ForwardLogin`) establishes a session
from a trusted identity header set by a forward-auth proxy (Authelia,
authentik, Traefik ForwardAuth), falling back to normal cookie login.

> **Operator setup** — env vars, reverse-proxy requirements (header stripping, GReader and image-proxy bypass), logout behavior: [README.md → Authentication & SSO](README.md#authentication--sso).

- **Trust model:** the TCP peer IP (never `X-Forwarded-For`) must be in
  `RDRS_TRUSTED_PROXY_NETWORKS`; otherwise the header is ignored. Fails closed
  on untrusted peer, missing header, unparsable peer, or DB error.
- **Username mapping:** matched against existing usernames; no schema change.
  Existing password accounts gain forward-auth login automatically.
- **JIT creation:** with `RDRS_AUTH_PROXY_USER_CREATION=true`, unknown users get
  an account with sentinel hash `"!"` (no password login possible). The count,
  insert and default category share one transaction, so two concurrent first
  logins cannot both become admin.
- **Group → role sync:** with both `RDRS_AUTH_PROXY_GROUPS_HEADER` and
  `RDRS_AUTH_PROXY_ADMIN_GROUP`, role is recomputed and persisted on every
  forward-auth login; the IdP is authoritative.
- **`RDRS_DISABLE_LOCAL_AUTH`:** hides the password form, `POST /api/session` →
  403. GReader `ClientLogin` (`/accounts/ClientLogin`) and passkeys unaffected.
- **Scope:** browser page routes only; machine endpoints and the PWA paths
  (`/static`, `/favicon`, `/health`, `/sw.js`, `/offline`, GReader, `/api`,
  `/events`) are skipped. Skipped when a valid session cookie is present; an
  invalid cookie never blocks forward-auth.

**Logout mechanics:**
- Sign Out always clears the session cookie (under both names) and deletes the
  server session. It succeeds even if the session is already gone (idle
  timeout, revocation, new `RDRS_SECRET`): `DELETE /api/session` and
  `POST /logout` still clear cookies and send `Clear-Site-Data` rather than 401.
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
- `/user-settings` lists non-expired sessions as cards (shared with the API
  token list): full `user_agent`, `ip_address`, created/last-active/expires.
  The row `id` reaches the template; `session_token` never does.
- `POST /user-settings/sessions/{id}/revoke` is `user_id`-scoped, reports
  "already gone" from the affected-row count, and refuses the caller's own
  session (card shows "This device"; handler re-checks).
- `POST /user-settings/sessions/revoke-others`: count goes to the flash and to
  the audit event's `count`.
- `POST /user-settings/api-tokens/revoke-all` works the same way.

**Session metadata:**
- `user_agent`, `ip_address`, `last_seen_at` are `NOT NULL`, captured at every
  login site (`POST /api/session`, forward-auth, passkey, GReader
  `ClientLogin`).
- Client IP via `auth.ClientIP`: honours `X-Forwarded-For`/`X-Real-IP` only
  when the TCP peer is trusted (`Config.IsTrustedPeer`, same as forward-auth).
  Reads `X-Forwarded-For` right-to-left, taking the right-most entry that isn't
  a trusted proxy (append-mode proxies add hops on the right; left-most would
  be client-spoofable), then `X-Real-IP`, then the peer.
- `last_seen_at` is bumped on authenticated requests, throttled to once per
  minute per session (`models.TouchLastSeen`).
- Migration `0002_add_session_metadata` **drops and recreates `session`**:
  upgrading past it signs everyone out.

**Auth-mode indicator:** the sidebar shows an **SSO** pill when the request came
via forward-auth (computed per request; `via_forward_auth` on the principal and
the sidebar payload). `/settings` shows forward-auth config; it is
**admin-only**, passes `DATABASE_URL` through `config.RedactDatabaseURL`, and
the sidebar hides its link for non-admins.

## Services

### Feed Synchronization

**Background Scheduler** (`worker.Sync`): assigns feeds to 60 one-minute
buckets by URL hash (indexed `bucket` column; the hash is SipHash-1-3 with zero
keys in `internal/siphash`, so every URL keeps the bucket already stored) and
refreshes the bucket of the current minute every minute. A bucket in progress
is finished on shutdown. Missed ticks run back to back instead of being
dropped.

**Sync Logic** (`internal/feedsync`): conditional GET (etag /
if-modified-since); `internal/feedparse` handles RSS, Atom, RDF and JSON Feed
with charset decoding, and `internal/datetime` parses odd timestamps (Chinese
dates); new entries are inserted in one transaction with the fetch result,
duplicates skipped, tombstoned guids skipped. Feeds in a bucket refresh in
groups of 4. A single feed's fetch, retries, icon and writes are bounded by
90 s.

**Timestamps on `/feeds`:**
- `fetched_at` — written on every attempt.
- `feed_updated_at` — the **latest** of the feed's own timestamp, the newest
  entry date and the HTTP `Last-Modified`, so a feed with only `Last-Modified`
  isn't called stale. Written via `COALESCE($5, feed_updated_at)`, so a `304`
  or failure never clears it.
- The freshness grade (`server/timefmt.go`, `freshMaxDays` 30 /
  `warningMaxDays` 90) falls back to `fetched_at`. The Stale filter matches the
  stale band only. The constants are passed into the page, which explains the
  rule, rather than retyped.
- No failure counter or auto-disable: `fetch_error` holds the last error until
  the next success.

### Entry Retention

**Retention Worker** (`worker.Retention`):
- Opt-in per user via `user_settings.retention_read_days` (`0` = off).
- Every 24h, prunes read, non-starred entries older than the window, in batches.
- Each pruned entry gets an `entry_tombstone` (`feed_id`, `guid`) so sync won't
  re-insert it; tombstones cascade with the feed.
- SQLite maintenance: a tick that pruned runs `DB.RunMaintenance` (planner
  stats, VACUUM when the freelist reaches 20% of the file, truncating WAL
  checkpoint); a tick that pruned nothing still refreshes stats. The first
  (immediate) tick skips it since opening the database just did.

**Cleanup Worker** (`worker.Cleanup`, hourly) deletes expired summaries,
sessions and API tokens, and feed images whose feed was deleted (`image` has no
foreign key). **`worker.BackfillContentText`** fills `entry.content_text` for
rows that predate it, once per start and resumable.

### Content Processing

**HTML Sanitization** (`internal/sanitize`): an allowlist sanitizer on top of
`golang.org/x/net/html` (always runs, ungated). Pipeline: pre-passes, allowlist
clean, post-pass.
- Drops `aria-hidden="true"` subtrees first (the allowlist strips
  `class`/`style`, so hidden-by-CSS markup like Shiki line-number gutters would
  surface as text); an article that would be left empty is returned untouched
- Promotes lazy-loaded image URLs (`data-src`, `data-lazy-src`,
  `data-original`)
- Injects `width`/`height` from `data-original-width`/`-height` or inline
  `style` when missing
- Strips tracking params (`utm_*`, `mtm_*`, `fbclid`, …), tracking domains
  (`pixel.*`, `analytics.*`, …) and paths, and 1x1 pixels
- Resolves relative image URLs against the entry's base URL; an image that
  cannot resolve to http(s) is dropped
- Routes every image through the signed image proxy and tags it
  `data-img-state="loading"` (CSS skeleton; broken-image fallback with `alt`
  on error)
- Each pre-pass is fronted by a cheap case-insensitive substring gate. **A gate
  must remain a superset of its pass's trigger**;
  `TestGatesAreSupersetsOfThePassesTheyFront` enforces this. The passes are
  presentation, not security
- `internal/htmldepth` works around `x/net/html` refusing documents with more
  than 511 open elements

**Full Content Extraction** (`internal/readability`):
- Fetches the article and extracts main content; fetched through the
  SSRF-guarded `fetch.Fetcher`
- Stored **raw** in `entry.full_content` and sanitized per render (sanitizing
  signs proxy URLs with `RDRS_SECRET`, which a stored copy would outlive)
- Persists across refreshes/tabs/no-JS; `GET /entries/{id}/fragment?view=original`
  shows the feed's version; re-posting the action refreshes it
- Not mirrored into `content_text` (search covers feed content only)

### Image Proxy (`internal/imageproxy`, `server/media.go`)

Proxies external images for privacy and to avoid mixed content.
- Signed URLs: `/api/proxy/image?url=...&s=...`, HMAC-SHA256 truncated to 8
  bytes, verified in constant time before fetching.
- Default `Cache-Control: public, max-age=86400` (an origin's non-empty value is
  mirrored), `ETag` = the signature; matching `If-None-Match` → `304` without
  refetching (checked *after* signature verification).
- `GET /api/feeds/{id}/icon` uses `private, max-age=86400`: it is per-user
  (scoped to the caller's categories) and sets its own `Cache-Control`, so
  `NoStoreForAuthenticated` adds no `Vary: Cookie`; `public` would leak icons
  across users via shared caches. The proxy stays `public` because its URLs
  carry no per-user meaning.

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
  searches).
- **Load More** — `?fragment=1&after=<cursor>` appends before `#load-more`. Page
  size is the user's `entries_per_page`, read per request and clamped to
  `MinEntriesPerPage..MaxEntriesPerPage` (it reaches a SQL `LIMIT`).
- **list refresh** — `?fragment=1` (no cursor) re-renders `[data-entries-list]`
  and the "Mark N matching" slot, leaving the search box alone. The search
  drawer sits above `.list-pane-header` and renders open when `q` is present.
  Searching hides "Mark Above as Read" (the `A` shortcut still works). Also
  backs **Mark Above as Read** and the **"Mark as Read..." dropdown** via
  `refreshEntriesList()`, which re-renders in place and raises
  `rdrs:sidebar-stale` instead of reloading.
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

`GET /events` (`server/events.go`) streams per-user events. Mutations call
`events.Bus.EmitSidebar` / `EmitSummary` (`internal/events`). Each open tab has
a subscription with a 256-event buffer; a slow subscriber loses its oldest
event and is sent a resync nudge (refetch) instead of blocking the sender.
A 15 s keep-alive stops proxies closing an idle stream. `installSse()`
(`static/js/live-updates.js`, started from `app.js`) handles:

- **`sidebar`** — refetch `/api/sidebar` to update badges.
- **`summary`** — `{entry_id, status}`; updates the row badge and, if open,
  swaps `GET /entries/{id}/summary/fragment` into `#rp-summary-container`.

Graceful shutdown cancels the streams' base context and closes the bus, so
shutdown never waits on an open tab. `/events` is mounted outside the core
middleware (see [Router](#router-internalserverroutesgo)).

### Caching (`internal/cache`)

All caches are in-memory, live for the process, and are built on one bounded
TTL map (oldest insertion evicted first). Writers invalidate explicitly; the
TTL bounds the damage of a missed call.

- **Sidebar cache** (`cache.Sidebar`) — per-user sidebar chrome (theme,
  categories, unread counts), excluding session-specific fields (e.g.
  masquerade flag). 10 000 users, 60 s TTL; busted by handlers, sync, and the
  summary worker. Each slot has a generation that `Bust` bumps (leaving a
  tombstone); `BeginRead` captures it before reading and `InsertIfCurrent`
  publishes only if no bust landed since, so a bust during population isn't
  overwritten by a stale value. `RDRS_DISABLE_SIDEBAR_CACHE` (set to anything)
  disables it (E2E only — it seeds the database directly, bypassing bust
  hooks). The Today count depends on the clock, not just on data, so a slot
  records the user's timezone and local day and `Chrome.IsCurrent` rejects it
  once that day has passed.
- **Summary state** (`cache.Summary`) — pending / processing / completed /
  failed state of summary jobs (1000 entries, 24 h), so a page shows progress
  without a database row for unfinished work.
- **Admin database stats** (`cache.AdminDBStats`) — the one non-per-user cache:
  site-wide `/statistics` figures (`COUNT(*)` over `entry` and
  `entry_tombstone`, page-count PRAGMAs) in one slot. Never busted; the 60 s
  TTL is the whole invalidation strategy.

### Progressive Web App

Installable and offline-tolerant without leaving SSR. By default the browser
stores nothing of the reader's; offline reading is the only opt-in exception,
and turning it off restores that property.

**Manifest.** `static/manifest.webmanifest`, linked from the layout with `?v=`,
served under `/static/` (skipped by session, CSRF and forward-auth layers, so no
`Set-Cookie`). Served as `application/manifest+json` (the fallback type would be
rejected under `nosniff`). `start_url` and `scope` are absolute, else scope
would become `/static/`.

**Icons.** `static/icons/` holds committed PNGs (`icon-192.png`,
`icon-512.png`, `maskable-icon-512.png`) next to the favicons; all are embedded
with the rest of `static/`.

**Service worker.** `static/js/sw.js`, served at `/sw.js`
(`serveServiceWorker`; root scope sees navigations). Unstamped URL (a stamp
would register a new worker per deploy), so the build version is in the body
and the response is `no-cache`. Registered by `static/js/pwa.js` from the app
layout, so `/login` and `/setup` never register one.

**Worker caching rules** — an allowlist over its shell cache
(`rdrs-shell-<version>`), since the Cache API ignores `no-store` / `Vary: Cookie`:

- **Navigations** — network-first with navigation preload; fallback to the
  saved library for `/` and `/entries/*`, else the precached `/offline`. Never
  stored.
- **Same-origin `GET /static/`** — cache-first, populated on use; cache is keyed
  by build version and dropped on activate. Disabled for dirty builds (the
  server substitutes whether the worker may cache `/static/`, derived from the
  same `-dirty` rule as the asset `Cache-Control`).
- **Feed icons and proxied images** — network-first, falling back to the
  offline cache; never populated here (that budget is the reader's).
- **Everything else** (`/api/*`, `/events`, fragments) — passthrough, never
  stored.

Precache is just `/offline` and `app.css`; the rest of the shell is only reached
via navigations, which fail offline anyway.

**Offline page.** `GET /offline` renders with no auth and no user data,
`public, max-age=3600`. That header stops `NoStoreForAuthenticated` stamping
`no-store` and stops `SlideSession` appending a cookie. `/sw.js` and
`/offline` are in the forward-auth and session-sliding skip lists, and the
anonymous-session layer skips the PWA paths too.

### Offline reading

Opt-in via `user_settings.offline_keep` (entry budget; `0` default = store
nothing).

- **Stored markup is the server's.** The client mirrors
  `GET /entries/{id}/fragment` — one renderer, no client templating.
- **Prefetch doesn't mark read.** Mirroring uses `?offline=1`, which dispatches
  no write. (`Sec-Purpose` can't be used: it is forbidden to `fetch()`.)
- **What to hold:** `GET /api/offline/manifest` returns ids, per-entry
  `updated_at`, the budget, and an opaque `cache_key` — no content. The set is
  `entry.ListOfflineSet`: newest unread up to budget, starred filling the rest.
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
- **Library page:** `GET /entries/offline` lists the whole saved set — no Load
  More, search or bulk actions. Linked by `<rdrs-sidebar>` and the scriptless
  nav whenever `offline_keep > 0` (read from `data-offline-keep` beside
  `data-offline-key`). The worker serves its cached copy for dead `/` and
  `/entries/*` navigations.
- **Per-reader, cleared on sign-out.** Cache name
  `rdrs-offline-<secret.OfflineID(user_id)>` (opaque, since page JS sees it).
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

**Linkding** (`internal/save`): saves entries as bookmarks; configured per
user. A Linkding-side failure comes back as a result with `Success` false, so
one integration cannot fail the others. It uses its own HTTP client
(user-typed address), not the SSRF-guarded fetcher.

**Open tracking** (`internal/pixel`, `GET /p/{token}`): a signed 1x1 GIF is
appended to the rendered entry content (after sanitizing, never before: the
sanitizer strips 1x1 images) so the server learns which entries a client
rendered (`entry_open`). The endpoint answers the same GIF for every request,
valid or not.

### AI Summarization

Kagi Universal Summarizer integration:
- `summarize/kagi.go` — API client (own HTTP client)
- `summarize/worker.go` — job queue (100 jobs; a full queue fails the enqueue
  after 5 s) and background worker
- `cache.Summary` — in-memory state of in-flight and recent jobs
- `worker.Cleanup` — periodic cleanup of expired summaries (see above)
- On start, `summarize.RecoverIncomplete` re-queues jobs left unfinished

**Flow:** request → check cache, then DB → otherwise queue to worker → worker
calls Kagi and stores in `entry_summary` → cached and returned.

- **Cancellation:** each running job has a cancel function in a `Cancels`
  registry keyed by `(user_id, entry_id)`; `POST /entries/{id}/summarize/cancel`
  cancels it (aborting the HTTP request) and deletes the record.
- **Timeout:** 90 s hard timeout per Kagi request; expiry marks it `failed`.
- **Status:** pending, processing, completed, failed (with error message).

## Security

### Signing & the root key (`internal/secret`)

One root key — `RDRS_SECRET`, or random at boot — backs every signature. Each
use has a domain-separation prefix (`image:`, `greader-token:`, `session:`,
`csrf:`, `audit:`, `offline:`, `invite:`, `pixel:`, `service-tokens:`, `flash:`,
`setup:`) so a value for one purpose can't be replayed as another (without it,
the CSRF token would equal the session cookie's signature).

- **Session cookie:** `<session_token>.<hmac>`. Every authentication path reads
  it through `auth.Cookies.SessionToken`, verifying before the DB lookup; a
  leaked `session.session_token` is useless without the key.
- **Image-proxy URLs** and the **GReader post token** use their own domains
  (`internal/imageproxy`, `internal/greader`).

Rotating the key (including restarting without `RDRS_SECRET`) ends all
sessions and breaks image-proxy URLs cached by GReader clients until re-sync.
GReader `ClientLogin` API tokens (`rdrs_gr_` prefix) are matched against the
database, not signed.

### CSRF protection

Two independent lines.

- **First line — `middleware.CrossSiteGuard`**, stateless, header-only, over
  the whole router. Follows tower-http's CSRF semantics, not Go's
  `http.CrossOriginProtection`.
  - Unsafe methods (TRACE counts as unsafe here) require `Sec-Fetch-Site` of
    `same-origin` or `none`; `same-site` is rejected (sibling subdomains/ports
    get the cookie under `SameSite=Lax`).
  - Without `Sec-Fetch-Site` (Safari < 16.4): a present `Origin` authority
    (host *and port*) must byte-match `Host`; `Origin: null` is rejected.
    Scheme is ignored (works behind TLS termination), but a proxy that drops the
    port from `Host` (nginx `$host` on a non-default port) breaks it — see
    README → Production Notes.
  - Neither header (native GReader clients, `curl`, server-to-server; all
    bearer-authenticated) passes.
  - A refusal is 403 with an empty body, logged as `csrf.cross_site` with
    `check=sec_fetch_site` or `check=origin_fallback`.
- **Second line — synchronizer token** (`middleware.CSRFGuard`).
  - Token = `secret.DeriveCSRF` (HMAC of the session token under `csrf:`): no
    column, no query.
  - Unsafe methods must echo it via `X-CSRF-Token` or a `_csrf` urlencoded field
    (body buffered, capped at 1 MiB, and rebuilt for the handler).
  - Delivered as a readable `csrf_token` cookie; `static/js/csrf.js` adds it to
    same-origin `fetch` and native POST forms. The cookie is never the
    credential; only the derived MAC is.
  - Multipart bodies are **not** generally waved through: only `/feeds/import`
    (OPML) passes the guard, and that handler validates the field itself.
  - GReader paths are **not** exempt: a browser calling `/reader/api/0/*` with
    an ambient cookie keeps its token requirement. Only `ClientLogin` is skipped
    (suffix match, since it is also mounted under `/api/greader.php`) — a forged
    call needs the attacker's own credentials, and login-CSRF is caught by the
    first line.
  - Requests with **no** (valid) session cookie pass (a forged action must ride
    the victim's cookie).
- **Anonymous sessions** (`middleware.AnonymousSession`). Logged-out
  visitors to HTML pages get a signed `session_token` with no `session` row
  (unauthenticated) plus its `csrf_token`, so login forms carry a token.
  Layered inside `ForwardAuth` (a real session's `Set-Cookie` wins); skipped for
  cacheable assets and machine APIs to avoid cookie-poisoning shared caches.
- **Keeping cookie names in step.** Browsers may hold both `csrf_token` and
  `__Host-csrf_token` (on `Secure` deployments). Rules:
  - `csrf.js` prefers `__Host-`; `Cookies.SessionToken` tries the `__Host-` name
    first and then the plain one, so flipping `RDRS_COOKIE_SECURE` logs no one
    out.
  - `AnonymousSession` validates the cookie against
    `DeriveCSRF(secret, session_token)`, re-mints on mismatch, and expires the
    other name.
  - Both guards log a warning on rejection (`csrf.cross_site`, `csrf.mismatch` —
    the latter identifies the session only by `secret.AuditID`).
  - Self-healing is required: logout itself is behind `CSRFGuard`.

### Response security headers

`middleware.SecurityHeaders` and `middleware.HSTS` wrap the router just inside
`RequestLog`, and both leave existing headers alone (reverse proxy wins):

- **`SecurityHeaders`** — always: `Content-Security-Policy`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy`, `Permissions-Policy`,
  `X-Frame-Options: DENY`, `Cross-Origin-Opener-Policy: same-origin`. Fixed in
  source.
- **`HSTS`** — `Strict-Transport-Security`, installed only when HTTPS per
  `Config` (`Config.HSTSHeader`: `RDRS_HSTS`, `RDRS_HSTS_MAX_AGE`,
  `RDRS_HSTS_INCLUDE_SUBDOMAINS`). Never includes `preload`.

Outside the core stack because `ForwardAuth` and both CSRF guards return early
on several paths (session-minting redirect, "not authorized" redirect, 403s).
No path skip list — `/static`, `/health`, the image proxy and the SSE stream
get them too.

**Strict CSP:**

```
default-src 'self'; script-src 'self'; style-src 'self';
img-src 'self' data:; font-src 'self'; connect-src 'self'; object-src 'none';
base-uri 'self'; form-action 'self'; frame-ancestors 'none'
```

No `'unsafe-inline'`: **no inline `<script>`, `on*=` attributes, `style=`
attributes, or inline `<style>`.** These fail silently in the browser, so a
unit test (`internal/views/views_test.go`) scans every rendered view for inline
handlers and styles, and a runtime audit covers the rest.

Script replacements:
- Page-specific script → module in `static/js/`, loaded via `src`.
- `onsubmit="return confirm(…)"` → `data-confirm="…"` on the `<form>`.
- `onchange="this.form.submit()"` → `data-submit-on-change` on the `<select>`.
- `onerror="this.style.display='none'"` → `data-hide-on-error` on the `<img>`.

Listeners live in `static/js/behaviors.js`; flash dismiss is in
`components/rdrs-flash.js`. `<script type="application/json">` blocks are
unaffected.

Style replacements:
- Static declaration → class in `static/css/app.css` (`.form-inline`, `.text-xs`, …).
- `style="display:none"` toggled by JS → `hidden` attribute / `.hidden` property.
- Per-datum geometry on /statistics → a `pct-N` class (0–100 scale at the end of
  `app.css`); `--pct` drives `height` in `.stats-bar` and `width` in
  `.stats-progress-fill`.
- Shadow-DOM styles → constructable stylesheet via `adoptedStyleSheets` (see
  `components/rdrs-kb-help.js`).

Setting `element.style` from script is fine — CSP polices markup, not the CSSOM.

**Runtime audit:** `e2e/cmd/cspaudit` walks the app in Chromium and fails on
any `securitypolicyviolation` (runtime markup, shadow-root `<style>`,
cross-origin `@import`/fonts, unexpected `img-src`). It ends with a **positive
control** (a planted `style=` that must be blocked). CI runs it; locally
`cd e2e && go run -mod=vendor ./cmd/cspaudit`.

Deliberate omissions: **no `Cross-Origin-Resource-Policy`** (would block
absolute `/api/proxy/image` URLs in native GReader webviews); **no
`publickey-credentials-*` in `Permissions-Policy`** (default `self` is what
passkeys need). `Referrer-Policy` is `strict-origin-when-cross-origin`, not
`no-referrer`, because entry-action redirects use the same-origin `Referer`.

### Password Hashing

Argon2id (`golang.org/x/crypto`): 19 MiB memory, 2 iterations, parallelism 1.
Verification reads the cost from the stored PHC string and refuses one whose
memory would exhaust the process; the `"!"` sentinel never verifies.
`RDRS_FAST_HASH` selects minimal cost for tests and CI; never set it in
production.

### Password Policy

`auth.ValidatePasswordStrength` gates every *new* password (setup, change,
invite redemption); existing passwords are never re-checked or force-rotated.

1. **Length** 15–128 **characters** (not bytes). 15 is NIST SP800-63B's floor
   without a second factor. Over-long passwords are rejected, never truncated.
2. **zxcvbn** score ≥ 3 (of 0–4), with the username as `user_input`
   (`internal/zxcvbn`, a rune-indexed fork that keeps the verdicts of the
   earlier Rust crate).

No breached-password blocklist: common-password lists are almost entirely
shorter than 15 characters, and zxcvbn catches long structured ones
(`passwordpassword`). Revisit if a second factor lowers the minimum to 8.

zxcvbn is expensive on long input, so call sites run it **behind** the rate
limiter. Only the pass/fail gates; the guess count is never shown.

### Session Management

- Sliding expiry: 7-day TTL, extended when less than half remains; absolute cap
  90 days from creation (`models.SessionExpiryDays`, `SessionAbsoluteMaxDays`).
- Token rotation (OWASP "Renewal Timeout") rides the same trigger (~3.5-day
  token life). Authentication asks for it (`auth.RequestRotation`);
  `SlideSession` performs it on the way out, skipping publicly cacheable
  responses (e.g. the feed-icon route) where the new cookie wouldn't reach the
  client. Rotation matches on the old token, so concurrent requests can't
  chain rotations.
- The old token stays valid for `RotationGrace` (60 s) via
  `session.previous_token`; session deletion matches it too.
- Cookies: `HttpOnly`, `SameSite=Lax`, `Max-Age` = absolute cap. `Secure` from
  `Config.CookieSecure` (`RDRS_PUBLIC_BASE_URL` scheme, overridable via
  `RDRS_COOKIE_SECURE`); under `Secure` the names carry the `__Host-` prefix.
  All login paths build the cookie through `auth.Cookies.Session`.
- **Re-authentication** (OWASP "Reauthentication After Risk Events"):
  - `session.last_authenticated_at` records the last credential proof;
    `Authenticator.RecentlyAuthenticated` requires it within `ReauthWindow`
    (5 minutes). Missing counts as stale.
  - Guards passkey registration (at ceremony *start*, since challenges are
    single-use) and removal — a password change doesn't revoke passkeys.
  - Guards admin actions on other accounts (`needsRecentAuthentication` in
    `server/admin.go`). Ending a masquerade is **not** guarded (the demanded
    password would be the impersonated account's; it's a de-escalation anyway).
  - `POST /api/session/reauth` reopens the window, sharing the
    `password change` rate-limit budget. `POST /admin/reauth` is its form twin
    for the no-JS admin panel. While masquerading it verifies the *original*
    admin's password (`session.original_user_id`).
  - Forward-auth sessions are exempt (identity re-asserted each request; JIT
    accounts have unverifiable hashes).
- **Masquerade:** start and stop rotate the session token in the same `UPDATE`
  that swaps `user_id` (privilege change), and reissue **both** session and
  CSRF cookies (CSRF derives from the token).
  While masquerading, the target's credentials stay out of reach: session
  management, passkey routes and the Linkding/Kagi service forms are refused
  (a passkey would outlive the masquerade; a changed `api_url` would receive
  the stored token). `DeleteUserSessions` also removes sessions whose
  `original_user_id` is the user, so disabling an admin ends their masquerade.
- **Audit** (`internal/audit`): session and credential lifecycle events
  (creation, renewal, token rotation, destruction, masquerade, re-authentication,
  passkey added/removed, invites, rate limiting, failed logins) log under
  target `rdrs::audit`, with sessions identified only by `secret.AuditID`. A new
  credential path must emit an audit event.

### Authentication: deliberately not done

Of nine OWASP Authentication Cheat Sheet findings, these were **declined**:

- **Re-hash on login when Argon2 params change.** Belongs with the change that
  raises the params. When written: compare *upward only* and disable under
  `RDRS_FAST_HASH`, or test logins will downgrade hashes.
- **TOTP second factor.** Passkeys and forward-auth already provide strong
  auth; TOTP means owning recovery codes and lost-device flows with no support
  desk. A lighter alternative: a per-user "disable password login once a
  passkey is enrolled" switch.

`PasswordMinLength` is 15 *because* there is no second factor; adding one
would allow 8 and make a breached-password blocklist worth revisiting.

### Input Sanitization

- **Third-party credentials** (Linkding, Kagi) are encrypted at rest with
  `XChaCha20-Poly1305` under a key derived from `RDRS_SECRET`
  (`secret.Seal`/`secret.Open`). Legacy plain JSON stays readable and is
  sealed on next write. A *generated* `RDRS_SECRET` stores plaintext on purpose
  (`Config.ServiceTokenKey`). Undecryptable values are surfaced as such, not as
  "not configured".
- **Flash cookie** is HMAC-signed (`secret.DomainFlash`) by
  `middleware.SignFlashCookies` (sign on the way out, verify on the way in, so
  handlers need no secret). Payload is base64url so signed bytes equal checked
  bytes. Stops sibling apps/subdomains from injecting banners.
- **Image proxy** serves by **sniffed magic bytes**, not origin
  `Content-Type`; `application/octet-stream` is treated as unlabelled. SVG is
  allowed (`nosniff` + CSP neutralise scripts).
- All feed HTML sanitized (`internal/sanitize`); model-written summaries are
  reduced to inline prose markup.
- Parameterized SQL throughout, including dynamic filters.
- **SSRF protection** on every fetch of a URL the app didn't choose
  (readability, image proxy, feed discovery, feed sync, icon fetcher, OPML
  import), all via `fetch.Fetcher`, which checks:
  - the **URL** before the request (`fetch.ValidateURL`);
  - **every redirect hop** (max 5);
  - **every DNS answer** at dial time (no rebinding window).

  Blocked: loopback, private, link-local, CGNAT (`100.64/10`, Tailscale),
  benchmarking, multicast, reserved, IPv6 ULA/link-local, and IPv4-mapped
  forms. Opt hosts back in with `RDRS_FETCH_ALLOW_PRIVATE_HOSTS` (hostnames,
  IPs or CIDRs; a malformed entry fails startup); non-http(s) is always
  refused. Linkding and Kagi use their own clients (user-typed addresses).

## Deployment

### Docker

Multi-stage Dockerfile (minimal attack surface, cached layers):
1. **build** - `golang:bookworm`, cross-compiles a static binary
   (`CGO_ENABLED=0`, `-mod=vendor`, `-trimpath`) on the build platform, so no
   qemu or C toolchain; `GIT_VERSION` is stamped via `-ldflags`
2. **runtime** - `gcr.io/distroless/static-debian12` (CA certs and tzdata, no
   shell); `DATABASE_URL=/data/rdrs.sqlite3`, `RDRS_SERVER_BIND=0.0.0.0:8080`

### Production Considerations

See [README.md → Production Notes](README.md#production-notes).
