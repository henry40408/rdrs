# RDRS - RSS Reader in Rust

> A self-hosted RSS/Atom feed reader built with Rust.

[![CI](https://github.com/henry40408/rdrs/actions/workflows/ci.yml/badge.svg)](https://github.com/henry40408/rdrs/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/henry40408/rdrs/graph/badge.svg)](https://codecov.io/gh/henry40408/rdrs)
[![Release](https://img.shields.io/github/v/release/henry40408/rdrs)](https://github.com/henry40408/rdrs/releases/latest)
[![License](https://img.shields.io/github/license/henry40408/rdrs)](LICENSE.txt)
[![Rust toolchain](https://img.shields.io/badge/dynamic/toml?url=https://raw.githubusercontent.com/henry40408/rdrs/main/rust-toolchain.toml&query=$.toolchain.channel&label=rust%20toolchain&logo=rust)](https://www.rust-lang.org/)
[![Docker](https://img.shields.io/badge/docker-ghcr.io-blue.svg)](https://ghcr.io/henry40408/rdrs)
[![Casual Maintenance Intended](https://casuallymaintained.tech/badge.svg)](https://casuallymaintained.tech/)
[![Vibe Coded](https://img.shields.io/badge/vibe_coded-Claude-d97757?logo=anthropic&logoColor=white)](https://claude.com/claude-code)

Privacy-focused, lightweight, and designed for personal use.

| Light | Dark |
|-------|------|
| ![Unread list - Light](screenshots/unread-list.png) | ![Unread list - Dark](screenshots/unread-list-dark.png) |
| ![Keyboard shortcuts - Light](screenshots/keyboard-shortcuts.png) | ![Keyboard shortcuts - Dark](screenshots/keyboard-shortcuts-dark.png) |

## Features

- **Feed Management** - Subscribe to RSS/Atom feeds, organize into categories, OPML import/export
- **Reading Experience** - Mark read/unread, star entries, full-text search, keyboard shortcuts
- **Privacy Protection** - HTML sanitization, tracking URL removal, image proxy
- **Full Content Extraction** - Fetch complete article content using readability algorithm
- **AI Summarization** - Automatic article summaries via Kagi AI integration
- **WebAuthn/Passkey** - Passwordless authentication with passkey support
- **External Services** - Save entries to Linkding bookmark manager
- **Google Reader API** - Compatible with GReader clients (FeedMe, Read You, etc.)
- **Multi-User Support** - Role-based access control with admin panel
- **Session Management** - View active sessions (device, IP, last active) and sign out other devices from Settings; GReader API tokens are tracked and revocable separately from browser sessions
- **Installable (PWA)** - Add to your home screen or dock and run it in its own window; a lost connection shows RDRS's own offline page instead of the browser's error
- **Offline reading** - Optionally keep your newest unread entries, and your starred ones, readable on the train. Off by default: at zero, nothing you read is stored on the device
- **Docker Ready** - Single-binary deployment with all assets embedded, multi-platform container images

## Quick Start

### Using Docker (Recommended)

```bash
docker run -d \
  --name rdrs \
  -p 8080:8080 \
  -v rdrs_data:/data \
  -e RDRS_SECRET="$(openssl rand -base64 32)" \
  ghcr.io/henry40408/rdrs:latest
```

Visit `http://localhost:8080`, which opens the one-time setup page, and create
the administrator account.

> **`RDRS_SECRET`** — the one key rdrs signs everything with: session cookies,
> image-proxy URLs, and the Google Reader post token. If left unset, a random
> key is generated on each startup, so every restart signs every signed-in user
> out and breaks every image-proxy URL already cached by a GReader client until
> its next sync. Set it to a persistent value (e.g. `openssl rand -base64 32`)
> so both survive restarts.

### Building from Source

```bash
# Clone repository
git clone https://github.com/henry40408/rdrs.git
cd rdrs

# Build release binary
cargo build --release

# Run server
./target/release/rdrs
```

Visit `http://localhost:8080`. With an empty database this serves `/setup`,
the one-time page that creates the administrator account. It closes for good the
moment that account exists — **there is no public sign-up**. Everyone else is
added by an admin from `/admin`, who hands out a one-time link; see
[Adding people](#adding-people).

## Configuration

All configuration is done via environment variables.

> **Upgrading from a release before the `RDRS_` prefix?** Every rdrs-specific
> variable gained an `RDRS_` prefix, and `IMAGE_PROXY_SECRET` became
> `RDRS_SECRET`. The old names are **no longer read**, and rdrs **refuses to
> start** while any of them is still set, listing each one and its replacement.
>
> The refusal is deliberate. Ignoring the old names would start a perfectly
> healthy server on defaults — against an empty `rdrs.sqlite3` rather than your
> database, with signup off and a regenerated secret. Failing once, loudly, is
> the cheaper outcome.
>
> **Upgrading to a version without self-service registration?**
> `RDRS_SIGNUP_ENABLED` no longer configures anything and rdrs **refuses to
> start** while it is set — silently ignoring it would leave you believing a
> public sign-up form exists when the endpoint is gone. Remove it. Accounts are
> now created by an admin from `/admin`, who hands the new user a one-time link
> to choose their own password (see [Adding people](#adding-people)); the very
> first account is still created at `/setup` on an empty instance.
> `RDRS_MULTI_USER_ENABLED` keeps its meaning and now governs the admin's
> create-account form.
>
> **Upgrading to a version with session device/IP tracking?** The `session`
> table gained mandatory `user_agent`, `ip_address`, and `last_seen_at`
> columns. Since there is no sensible default for existing rows, the migration
> **drops and recreates the `session` table** — every signed-in user,
> including you, is logged out once and must sign back in after the upgrade.
>
> **Upgrading to a version with independent GReader API tokens?** `POST
> /accounts/ClientLogin` used to hand a GReader client the raw web
> `session.session_token` — a token leaked from an RSS reader app was a full
> session takeover. It now mints its own row in a new `api_token` table
> (prefixed `rdrs_gr_`, revocable from `/user-settings` independently of your
> browser sessions). This is a **breaking change**: every existing GReader
> client's stored token stops working once and the client must run
> `ClientLogin` again — mainstream clients (FeedMe, Read You) store your
> username/password and do this automatically, so real-world impact is small.
> There is deliberately no opt-out: a migration flag that keeps honouring the
> old tokens would in practice be left on forever, which is the same as not
> having made the change.
>
> Each account keeps at most 20 of these tokens; a new one past that evicts
> the oldest. `ClientLogin` mints a row per call, so a client that
> re-authenticates every sync instead of caching its `Auth=` token would
> otherwise grow the table without bound. Twenty is well beyond the handful of
> devices a real user syncs from.
>
> `DATABASE_URL` is the exception and keeps its bare name: it is a genuine
> cross-tool convention. The rest only looked generic — `USER_AGENT` and
> `SERVER_BIND` were rdrs's own names all along, which is exactly what made them
> collide with other services in a shared compose file. `RUST_LOG` and
> `NO_COLOR` are likewise untouched.

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | `rdrs.sqlite3` | Database location. A file path or `sqlite://` URL selects SQLite (zero-config default); a `postgres://` URL selects PostgreSQL. The backend is chosen once at startup. |
| `RDRS_SERVER_BIND` | `127.0.0.1:8080` | HTTP server bind address (`host:port`). Defaults to loopback so a bare-metal run is not exposed on all interfaces without opting in; the container image sets `0.0.0.0:8080` so a reverse proxy can reach it. |
| `RDRS_MULTI_USER_ENABLED` | `false` | Allow more than one account. Governs the admin's "Add an account" form; there is no public sign-up either way. |
| `RDRS_SECRET` | Auto-generated | Root HMAC key backing every signature rdrs produces — session cookies, image-proxy URLs, and the GReader post token — each domain-separated so a value minted for one use cannot be replayed as another. Set a persistent value (`openssl rand -base64 32`); a generated one changes on every restart, ending all sessions and breaking cached image-proxy URLs. |
| `RDRS_PUBLIC_BASE_URL` | - | Public base URL for generating absolute image proxy URLs in API responses (e.g., `https://rdrs.example.com`). If not set, relative paths are used (backward compatible). |
| `RDRS_COOKIE_SECURE` | Derived from `RDRS_PUBLIC_BASE_URL` | Send the session cookie with the `Secure` attribute (HTTPS only). Defaults to on when `RDRS_PUBLIC_BASE_URL` starts with `https://`, off otherwise — so an HTTPS deployment is secure without a second setting, while a plain-HTTP dev run keeps working. Set `true`/`1` to force it on when TLS terminates upstream and `RDRS_PUBLIC_BASE_URL` is unset; set `false`/`0` to force it off. Only those four values are accepted — anything else fails startup rather than silently disabling `Secure`. |
| `RDRS_HSTS` | Derived from `RDRS_PUBLIC_BASE_URL` | Send `Strict-Transport-Security` on every response. Defaults to on when `RDRS_PUBLIC_BASE_URL` starts with `https://`, off otherwise, mirroring `RDRS_COOKIE_SECURE`. Only `true`/`false`/`1`/`0` are accepted — anything else fails startup rather than silently guessing. HSTS is sticky: once a browser sees it, that browser refuses plain HTTP to this host for the whole `RDRS_HSTS_MAX_AGE`, and the server has no way to retract it instantly. Leave this unset (or `false`) for a plain-HTTP internal deployment — turning it on by accident can lock users out with no server-side fix. |
| `RDRS_HSTS_MAX_AGE` | `31536000` (1 year) | HSTS `max-age` in seconds. `0` is the documented recovery path for a mis-set HSTS declaration: it tells a browser that already cached the header to forget it, which is how you undo `RDRS_HSTS` having been on by mistake — plain omission does not do this, since a browser that already saw the header keeps enforcing HTTPS until `max-age` naturally expires. |
| `RDRS_HSTS_INCLUDE_SUBDOMAINS` | `true` | Append `; includeSubDomains` to the HSTS header. **Warning:** if `RDRS_PUBLIC_BASE_URL` is an apex domain (`example.com`) rather than a subdomain (`rdrs.example.com`), this forces HTTPS on *every* subdomain of that registrable domain, not just the one rdrs serves. Set `false` to scope the declaration to rdrs's own host. The header never includes `preload`: joining the browser preload list is effectively irreversible, so that opt-in is left to your reverse proxy. |
| `RDRS_LOGIN_RATE_LIMIT_ATTEMPTS` | `5` | Attempts allowed per client IP per window for each credential-endpoint class (password login, registration, passkey ceremonies, and changing a password) — each class has its own budget, so exhausting one never locks you out of another. Password login additionally charges a per-**account** budget of 4× this value over the same window, so a spray that rotates through addresses is still capped; it is deliberately wide enough that normal use never reaches it. A throttled request answers `429` with a `Retry-After` giving the seconds left in the window. `0` disables the limiter. |
| `RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS` | `60` | Fixed window length in seconds. Must be ≥ 1. |
| `RDRS_USER_AGENT` | `RDRS/...` | Custom user agent for feed fetching |
| `RDRS_WEBAUTHN_RP_ID` | `localhost` | WebAuthn Relying Party ID for passkey authentication |
| `RDRS_WEBAUTHN_RP_ORIGIN` | `http://localhost:{port}` | WebAuthn Relying Party origin URL |
| `RDRS_WEBAUTHN_RP_NAME` | `rdrs` | WebAuthn Relying Party display name |
| `RUST_LOG` | - | Log level filter (e.g., `info`, `debug`, `rdrs=debug`). When unset, defaults to `error,rdrs=info` (rdrs' own INFO logs are visible; other crates stay at ERROR). |
| `RDRS_LOG_FORMAT` | `full` | Log output format: `full`, `compact`, `pretty`, or `json`. Can also be set via `--log-format`. |
| `RDRS_AUTH_PROXY_HEADER` | - | Header carrying the username from a forward-auth proxy (e.g. `Remote-User`, `X-Forwarded-User`). Empty disables the feature. |
| `RDRS_TRUSTED_PROXY_NETWORKS` | - | Comma-separated CIDRs or bare IPs (e.g. `10.0.0.0/8, 192.168.1.5`). The TCP peer IP must fall within one of these for the identity header to be trusted. Required when `RDRS_AUTH_PROXY_HEADER` is set. Also determines how the credential rate limiter identifies clients, so it should be set whenever rdrs runs behind a reverse proxy — not only when `RDRS_AUTH_PROXY_HEADER` is set. Without it, all requests appear to come from the proxy and share one rate-limit bucket. |
| `RDRS_AUTH_PROXY_USER_CREATION` | `false` | When `true`, JIT-create a local account for an unknown proxy-provided username instead of redirecting to `/login`. |
| `RDRS_AUTH_PROXY_GROUPS_HEADER` | - | Header carrying comma-separated group names from the proxy (e.g. `Remote-Groups`). |
| `RDRS_AUTH_PROXY_ADMIN_GROUP` | - | Membership in this group grants the admin role, synced on every forward-auth login. Active only when `RDRS_AUTH_PROXY_GROUPS_HEADER` is also set. |
| `RDRS_DISABLE_LOCAL_AUTH` | `false` | Hides the browser password form and rejects `POST /api/session` with 403. Does not affect GReader API or passkey auth. Requires `RDRS_AUTH_PROXY_HEADER`. |
| `RDRS_AUTH_PROXY_LOGOUT_URL` | (unset) | When set, Sign Out redirects the browser here (e.g. the Authelia logout URL) to end the SSO session. When unset, a forward-auth Sign Out clears the local session but stays put and warns the user to log out at the proxy instead — the proxy header would just re-authenticate them, so the app does not pretend the logout succeeded. |

> **Deploying behind a domain?** `RDRS_WEBAUTHN_RP_ID` and `RDRS_WEBAUTHN_RP_ORIGIN`
> default to `localhost` and **must** be overridden to your public host (e.g.
> `RDRS_WEBAUTHN_RP_ID=rdrs.example.com`,
> `RDRS_WEBAUTHN_RP_ORIGIN=https://rdrs.example.com`), otherwise the browser rejects
> passkeys. rdrs logs a startup warning while the RP origin still points at
> `localhost`, and the active values are shown on the Settings page.

### Structured Logging

Every log line rdrs emits carries an `event` field naming what happened, in
`domain.verb` form — `feed.sync_failed`, `retention.pruned`,
`summary.worker_started`, `shutdown.signal`. Values travel as their own typed
fields (`feed_id`, `entry_id`, `user_id`, `bucket`, `count`, `error`, …) rather
than being formatted into the message, so under `RDRS_LOG_FORMAT=json` they
arrive as real JSON values you can filter and aggregate on:

```json
{"timestamp":"…","level":"WARN","target":"rdrs::services::feed_sync",
 "fields":{"message":"feed sync failed","event":"feed.sync_failed",
           "feed_id":42,"error":"connection timed out"}}
```

The message itself stays static, which is what makes `event` worth having —
grouping by `feed.sync_failed` works, substring-matching a message that
interpolates a different feed id every time does not. The tracing `target` is
the module path, so `RUST_LOG=rdrs::services::feed_sync=debug` narrows to one
subsystem.

Two source-level tests (`tests/logging_test.rs`) enforce both halves of this:
every log call must set `event`, and no message may interpolate values.

### Request Timing

Every request is timed by `middleware/request_log.rs` and logged once, when the
response head is ready — carrying `method`, `route`, `status`, `elapsed` (human
readable) and `elapsed_ms` (a number, for aggregation):

| Event | Level | When |
| --- | --- | --- |
| `http.request` | DEBUG | Every request. Off under the default filter. |
| `http.slow_request` | WARN | The request took ≥ 1s. Visible by default, and carries `threshold_ms`. |

So a healthy deployment logs nothing per request while a slow one still stands
out. Turn the full stream on with `RUST_LOG=rdrs=debug`, or narrow it to just
this middleware with `RUST_LOG=rdrs=info,rdrs::middleware::request_log=debug`.

`route` is the *matched route template* (`/invite/{token}`), never the request
path: an invite token is a single-use credential that travels in the path, and
a log line outlives it. A request that matched no route is labelled
`<unmatched>` for the same reason — its path is attacker-controlled text.

The duration covers the server's own work up to the response head, so it
excludes streaming the body — which is why an `/events` SSE connection reads as
a fast request rather than a multi-minute one. Pair it with the database side,
which sqlx logs itself: `RUST_LOG=sqlx::query=debug` gives every statement with
its own `elapsed`, and `sqlx::query=warn` alone gives just the statements that
took over a second.

### Audit Logging

Session creation, renewal, and destruction; API-token issuance and revocation;
failed logins; rate-limited credential attempts; and masquerade start/stop are
logged as structured events under the `rdrs::audit` tracing target. Isolate
just that stream with `RUST_LOG=rdrs::audit=info` (combine with the default
`rdrs=info` via `RUST_LOG=rdrs=info,rdrs::audit=info`), or set
`RDRS_LOG_FORMAT=json` to ship the events to a SIEM.

Each event that identifies a session carries an `sid` field — a salted
HMAC-SHA256 hash of the session token, truncated to 16 hex characters, never
the token itself, so the log can never disclose an active session ID. The
salt is `RDRS_SECRET`, so rotating that key (including the implicit rotation
of a restart with no `RDRS_SECRET` set) breaks `sid` correlation with older
log lines — consistent with the fact that rotating that key already ends
every session.

## Authentication & SSO

RDRS supports three authentication methods that all work simultaneously by
default: local password, WebAuthn/passkeys, and **forward-auth (trusted-header)
SSO**. `RDRS_DISABLE_LOCAL_AUTH` is the only knob that narrows this set.

### Adding people

There is no registration form. The first account is created once at `/setup` on
an empty instance; after that, an admin adds accounts from `/admin`:

1. **Admin** enters a username and picks a role. The account is created but has
   no password, so nobody can sign into it yet.
2. **rdrs** returns a one-time link (`/invite/<token>`). It is displayed once
   and cannot be recovered afterwards — only a keyed hash of it is stored — so
   copy it there and then.
3. **The new user** opens the link, chooses their own password, and is sent to
   the sign-in page. The link is single-use and expires after **7 days**.

If a link expires or goes astray, issue another from `/admin` — that revokes the
previous one. The same button on an account that already has a password acts as
a **password reset**: rdrs has no self-service recovery (there is no email to
send it to), so an admin-issued link is the way back in. The old password keeps
working until the new link is redeemed, so issuing one cannot lock anybody out.
Redeeming any link signs that account out everywhere and revokes its GReader
API tokens.

Why not a sign-up form? Because an anonymous endpoint that accepts a username
inevitably answers whether that username is taken, and with no email to make
the answer ambiguous there is no way to hide it — whoever asks can simply try
to sign in with the password they just submitted. Removing the form removes the
question.

### Passwords

New passwords must be **15–128 characters**. Nothing else is required — no
mixture of cases, digits or symbols; spaces, punctuation and any script are all
fine, and a passphrase such as `correct horse battery staple` is a better
answer than a short scramble. The minimum follows NIST SP800-63B for accounts
without a second factor, which is what a password-protected rdrs account is
(passkeys here replace the password rather than supplement it). The maximum is
declared rather than discovered, and an over-long password is rejected outright
instead of being silently truncated.

Length alone does not make a password unguessable, so new passwords are also
scored with [zxcvbn](https://github.com/dropbox/zxcvbn) and refused when it
finds them trivially guessable — `passwordpassword`, `qwertyuiopasdfgh`,
`aaaaaaaaaaaaaaaa` and your own username with digits stuck on the end all clear
15 characters and none of them are worth having. The rejection quotes zxcvbn's
own explanation, so it says *what* is wrong rather than just "too weak". An
ordinary passphrase scores full marks, so in practice this never fires. Nothing
is sent anywhere: the estimator and its dictionaries are compiled into the
binary, like everything else.

Existing passwords keep working at whatever length they were set; rdrs never
forces a rotation. Changing one signs out every other browser session **and**
revokes every GReader API token, so connected RSS clients must sign in again.

### Admin actions ask for your password

Promoting, demoting, disabling, deleting, or viewing as another user all
require the admin to have confirmed their password in the last 5 minutes. Past
that, `/admin` shows a confirmation box; enter your password once and the
window re-opens for the whole panel. Ending a "view as" session never asks —
the password it would demand belongs to the account being impersonated.
Forward-auth (SSO) sessions are exempt throughout, since their identity is
re-asserted by the proxy on every request and their local account may hold no
usable password at all.

### Passkeys

"Login with Passkey" asks for no username: the sign-in challenge names no
credential, so your authenticator must be able to find its own passkey for this
site. Newly registered passkeys are always created that way.

> **Upgrading from a release before this changed?** Passkeys enrolled earlier
> were requested in a mode that lets an authenticator store them without making
> them findable. Where that happened — most security keys, some Windows Hello
> setups, and password managers that honour the request, Bitwarden among them —
> the passkey will no longer be offered at sign-in, and the browser just times
> out with "Authentication was cancelled or timed out." Passkeys held in iCloud
> Keychain or Google Password Manager are unaffected.
>
> To fix one, sign in with your password, **delete the old passkey** under
> Settings first, then register a new one. Registering before deleting fails:
> the site still lists the old credential as one to exclude, so your
> authenticator refuses to add a second passkey for the same account.

### Forward-Auth (SSO)

RDRS can delegate browser login to an external forward-auth proxy such as
Authelia, authentik, or Traefik ForwardAuth. The proxy authenticates the user
and forwards their identity in a trusted header; RDRS establishes a session from
it. Existing accounts are matched by username — no migration is required.

**Enable it** by setting (see the [Configuration](#configuration) table for all
fields):

- `RDRS_AUTH_PROXY_HEADER` — the username header your proxy injects (e.g.
  `Remote-User`).
- `RDRS_TRUSTED_PROXY_NETWORKS` — the CIDR(s)/IP(s) the proxy connects from. The
  identity header is trusted only when the TCP peer falls within this set, so it
  cannot be spoofed by a downstream client.

**Optional:** `RDRS_AUTH_PROXY_USER_CREATION` (JIT-create unknown users),
`RDRS_AUTH_PROXY_GROUPS_HEADER` + `RDRS_AUTH_PROXY_ADMIN_GROUP` (map a group to the admin
role on every login), `RDRS_DISABLE_LOCAL_AUTH` (hide the password form), and
`RDRS_AUTH_PROXY_LOGOUT_URL` (redirect Sign Out to your IdP's logout endpoint to also
end the SSO session; when unset, Sign Out clears the local session but warns the
forward-auth user to log out at the proxy instead, since the proxy header would
otherwise just re-authenticate them).

**Reverse-proxy requirements:**

1. The proxy **must** authoritatively set — and strip any client-supplied copy
   of — the identity (and groups) headers on every request before forwarding to
   RDRS. A downstream client able to inject these headers bypasses the trust
   model entirely.
2. The proxy **must** be configured to bypass forward-auth for the paths that
   are reached without a browser SSO session — they authenticate by their own
   means (or need none), so an SSO gate breaks them without adding any security:
   - The Google Reader API paths — `/accounts/ClientLogin`, `/reader/api/...`,
     and the FreshRSS-compatible `/api/greader.php/...` prefix — so native
     GReader clients (FeedMe, Read You, etc.) can still authenticate with their
     stored username and password. These paths authenticate via the GReader
     `ClientLogin` token, not the proxy header.
   - The signed image proxy `/api/proxy/...`. Proxied `<img>` requests are
     issued without the browser's SSO session (GReader clients render article
     images with no cookie at all), so an SSO-gated proxy path returns the login
     page instead of the image and **pictures break**. Exposing it is safe:
     every proxy URL carries an HMAC-SHA256 signature that RDRS verifies, so the
     signature — not the SSO session — authenticates each request.
   - The health endpoint `/health`, if you run liveness / uptime probes through
     the proxy. Probes carry no SSO session, so a catch-all policy makes them
     fail and can flap your orchestrator's health checks. (It needs no rule if
     you point probes straight at the container. Note `/health` returns the app
     status and build version unauthenticated, so scope the rule to your
     monitoring source if that disclosure matters.)

   - The PWA's public surface — `/static/...`, `/favicon...`,
     `/apple-touch-icon.png`, `/sw.js` and `/offline` — if you want the app to
     stay installable. The icons a web app manifest names are downloaded by the
     browser's install machinery rather than by the page, and that fetch is not
     governed by the `crossorigin` attribute on the `<link rel="manifest">`, so
     an SSO gate leaves the install prompt without an icon. These paths hold no
     user data and RDRS already serves them without a session: `/static`,
     `/favicon`, `/sw.js` and `/offline` are skipped by its session, CSRF and
     forward-auth layers outright (`SKIP_PREFIXES` in
     `src/middleware/forward_auth.rs`), and that cookie-free guarantee is
     precisely what makes `/static/` the only thing the service worker is
     allowed to cache.

   Everything else stays behind SSO. In particular, do **not** bypass
   `/api/feeds/{id}/icon`: that endpoint is guarded by RDRS's own session
   check, so a bypass would only strip a defense layer — native GReader clients
   still cannot load those icons (they have no browser session), which is a
   pre-existing limitation, not a forward-auth one.

   Example Authelia access-control rules:

   ```yaml
   access_control:
     rules:
       - domain: rdrs.example.com
         policy: bypass
         resources:
           - '^/accounts/ClientLogin$'
           - '^/reader/api/.*'
           - '^/api/greader\.php/.*'   # FreshRSS-compatible prefix
           - '^/api/proxy/.*'          # HMAC-signed image proxy (avoids broken images)
           - '^/health$'               # liveness / uptime probes (no SSO session)
           - '^/static/.*'             # PWA: manifest, icons, CSS and JS
           - '^/favicon.*'
           - '^/apple-touch-icon\.png$'
           - '^/sw\.js$'               # service worker
           - '^/offline$'              # its offline fallback page
       - domain: rdrs.example.com
         policy: one_factor            # everything else goes through SSO
   ```

> The trust model, middleware mechanics, and how the auth mode is detected per
> request are documented in
> [ARCHITECTURE.md](ARCHITECTURE.md#forward-auth-trusted-header-login).

## Usage

### Adding Feeds

1. Navigate to the Feeds page
2. Enter the feed URL (RSS/Atom feed or webpage with feed link)
3. RDRS will auto-discover the feed and fetch metadata

### Keyboard Shortcuts

The interface supports vim-style keyboard navigation for efficient reading.

### OPML Import/Export

- **Export**: Download all your feeds as an OPML file from Settings
- **Import**: Upload an OPML file to bulk-add feeds

### AI Summaries

RDRS can generate per-article summaries via the Kagi Universal Summarizer. This
is configured per user:

1. Go to Settings → Kagi Universal Summarizer
2. Paste your Kagi session link and choose a target language
3. Summaries are then generated on demand for entries

### Linkding Integration

Connect RDRS to your Linkding instance to save articles for later:

1. Go to Settings
2. Enter your Linkding URL and API token
3. Use the "Save" button on any entry

### Install as an App

RDRS ships a web app manifest and a service worker, so it can be installed from
the browser and run in its own window:

- **Desktop Chrome / Edge** — the install icon at the right of the address bar
- **Android Chrome** — ⋮ → *Install app*
- **iOS Safari** — Share → *Add to Home Screen*

The prompt appears once you are signed in. The sign-in page deliberately
registers no service worker, so a browser that never gets past it stays a plain
browser.

**Nothing of yours is stored on the device unless you ask for it.** Out of the
box the service worker keeps only the static assets (CSS, JavaScript, fonts,
icons) and a small offline page, so a navigation with no connection lands
somewhere legible rather than on the browser's error page.

Browsers only allow service workers on a secure origin, so installing works on
`http://localhost` for a local trial and needs HTTPS anywhere else. See
[Production Notes](#production-notes) for putting RDRS behind TLS.

### Offline reading

Set **Keep offline** in *Settings → Preferences* to the number of entries you
want available without a connection — `0`, the default, keeps none and stores
nothing. Anything above that mirrors your newest unread entries into the
browser, with your starred ones filling whatever budget is left over, and their
images along with them.

While you are online it syncs quietly in the background; **/entries/offline**
lists exactly what is currently saved, so you can check before you lose signal.
With the connection gone, opening the app lands you on that list and articles
open as usual. The dot beside the *rdrs* wordmark in the sidebar is the app's
own read on the connection: a steady muted green while the server is answering,
and once a request has failed, amber, captioned **Offline**, and slowly
breathing for as long as the app keeps retrying. It goes back to green and
holds still on the next request that succeeds.

What does *not* work offline is anything that has to reach the server: marking
read, starring, Load More, search, fetching full content, summarising. Those
controls grey out and stop responding rather than failing quietly, and nothing
is queued up for later — but you keep the page you were on instead of being
thrown back to an error.

Saved entries are scoped to your account and are deleted when you sign out, when
you switch accounts on the same browser, and when you set the number back to
`0`.

## Docker

### Docker Compose

```yaml
services:
  rdrs:
    image: ghcr.io/henry40408/rdrs:latest
    ports:
      - "8080:8080"
    volumes:
      - rdrs_data:/data
    environment:
      - RDRS_SECRET=your-secret-here
    restart: unless-stopped

volumes:
  rdrs_data:
```

### Building Docker Image

```bash
docker build -t rdrs:latest .
```

The Dockerfile uses multi-stage builds with a distroless base image for minimal
size and attack surface. The build-stage design is described in
[ARCHITECTURE.md](ARCHITECTURE.md#deployment).

### Production Notes

- Set `RDRS_SECRET` to a persistent value so sessions and image-proxy URLs
  survive restarts (otherwise it is auto-generated on each boot, ending every
  session and breaking cached image-proxy URLs).
- Mount the `/data` volume so the SQLite database persists.
- Put RDRS behind a reverse proxy for TLS termination.
- RDRS sends its own security headers on every response — `Content-Security-Policy`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy`, `Permissions-Policy`,
  `X-Frame-Options: DENY` and `Cross-Origin-Opener-Policy: same-origin`. These are
  fixed and have no environment variables; a header your reverse proxy already sets
  is never overwritten, so that is where to override one. `Strict-Transport-Security`
  is the exception, configured via the `RDRS_HSTS*` variables above.
- The CSP is strict on both scripts and styles (`script-src 'self'`, `style-src 'self'`,
  no `'unsafe-inline'` in either) and `img-src 'self'`
  assumes `RDRS_PUBLIC_BASE_URL` names the origin browsers actually use — the same
  assumption the `Secure` cookie flag and HSTS already make. Point it at a different
  host and article images, which are proxied through that URL, will be blocked.

## Development

### Prerequisites

- Rust (version pinned in `rust-toolchain.toml`; rustup installs it automatically)
- SQLite (bundled via rusqlite)

### Running Tests

```bash
cargo nextest run
```

### Project Structure

See [ARCHITECTURE.md](ARCHITECTURE.md) for detailed architecture documentation.

## Tech Stack

- **Web Framework**: Axum 0.8
- **Async Runtime**: Tokio
- **Database**: SQLite (rusqlite)
- **Templates**: Askama
- **Feed Parsing**: feed-rs
- **HTML Sanitization**: Ammonia
- **Content Extraction**: Readability

## License

MIT
