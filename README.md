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

- **Feed Management** - RSS/Atom feeds, categories, OPML import/export
- **Reading Experience** - Read/unread, stars, full-text search, keyboard shortcuts
- **Privacy Protection** - HTML sanitization, tracking URL removal, image proxy
- **Full Content Extraction** - Fetch complete articles via readability
- **AI Summarization** - Article summaries via Kagi
- **WebAuthn/Passkey** - Passwordless sign-in
- **External Services** - Save entries to Linkding
- **Google Reader API** - Works with GReader clients (FeedMe, Read You, etc.)
- **Multi-User Support** - Role-based access with admin panel
- **Session Management** - See active sessions (device, IP, last active), sign out other devices; GReader API tokens are revocable separately
- **Installable (PWA)** - Runs in its own window; shows an offline page instead of the browser error
- **Offline reading** - Optionally keep newest unread and starred entries on the device. Off by default (nothing stored)
- **Open tracking** - Optionally measure which feeds you actually read, per feed on Feeds and Statistics. Off by default; the 1×1 image is served by your own instance
- **Docker Ready** - Single binary with all assets embedded, multi-platform images

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

Open `http://localhost:8080` and create the administrator account on the
one-time setup page.

> **`RDRS_SECRET`** signs session cookies, image-proxy URLs, the flash cookie and
> the GReader post token, and **encrypts Linkding and Kagi credentials at rest**.
> Set a persistent value (`openssl rand -base64 32`). If unset, a random key is
> generated per startup: every restart signs everyone out, breaks image-proxy
> URLs cached by GReader clients, and credentials are stored unencrypted.
>
> At-rest encryption protects dumps and backups, not a compromised server.
> **Changing `RDRS_SECRET` makes stored credentials unreadable** (Settings says
> so); restoring the old value recovers them, so don't overwrite them hastily.

### Building from Source

```bash
git clone https://github.com/henry40408/rdrs.git
cd rdrs
cargo build --release
./target/release/rdrs
```

Open `http://localhost:8080`. An empty database serves `/setup`, which creates
the admin account and then closes for good — **there is no public sign-up**.
Others are added by an admin; see [Adding people](#adding-people).

## Configuration

All configuration is via environment variables.

> **Upgrade notes**
>
> - **`RDRS_` prefix:** every rdrs-specific variable gained an `RDRS_` prefix
>   and `IMAGE_PROXY_SECRET` became `RDRS_SECRET`. Old names are not read;
>   rdrs **refuses to start** while any is set, listing replacements.
>   `DATABASE_URL`, `RUST_LOG` and `NO_COLOR` keep their names.
> - **No self-service registration:** rdrs **refuses to start** while
>   `RDRS_SIGNUP_ENABLED` is set — remove it. Admins create accounts from
>   `/admin` (see [Adding people](#adding-people)); `RDRS_MULTI_USER_ENABLED`
>   now governs that form.
> - **Session device/IP tracking:** the migration **drops and recreates the
>   `session` table** — everyone is signed out once.
> - **Independent GReader API tokens:** `ClientLogin` now mints its own
>   `api_token` row (prefix `rdrs_gr_`, revocable in `/user-settings`) instead
>   of returning the web session token. **Breaking:** existing clients must run
>   `ClientLogin` again (FeedMe, Read You do this automatically). No opt-out.
>   Each account keeps at most 20 tokens; the oldest is evicted.

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | `rdrs.sqlite3` | File path or `sqlite://` URL → SQLite; `postgres://` URL → PostgreSQL. Chosen at startup. |
| `RDRS_SERVER_BIND` | `127.0.0.1:8080` | Bind address (`host:port`). The container image sets `0.0.0.0:8080`. |
| `RDRS_MULTI_USER_ENABLED` | `false` | Allow more than one account (governs the admin's "Add an account" form). No public sign-up either way. |
| `RDRS_SECRET` | Auto-generated | Root HMAC key for all signatures (sessions, image-proxy URLs, GReader post token), domain-separated. Set a persistent value; a generated one changes every restart. |
| `RDRS_PUBLIC_BASE_URL` | - | Public base URL for absolute image-proxy URLs in API responses (e.g. `https://rdrs.example.com`). Unset → relative paths. |
| `RDRS_COOKIE_SECURE` | Derived from `RDRS_PUBLIC_BASE_URL` | `Secure` session cookie. On when `RDRS_PUBLIC_BASE_URL` is `https://`, else off. Accepts only `true`/`false`/`1`/`0`; anything else fails startup. |
| `RDRS_HSTS` | Derived from `RDRS_PUBLIC_BASE_URL` | Send `Strict-Transport-Security`. Same default and accepted values as `RDRS_COOKIE_SECURE`. HSTS is sticky for `RDRS_HSTS_MAX_AGE`; leave off for plain-HTTP deployments or users get locked out. |
| `RDRS_HSTS_MAX_AGE` | `31536000` (1 year) | HSTS `max-age` in seconds. Set `0` to make browsers forget a mistakenly sent header (omitting it does not). |
| `RDRS_HSTS_INCLUDE_SUBDOMAINS` | `true` | Append `; includeSubDomains`. **Warning:** on an apex domain this forces HTTPS on every subdomain; set `false` to scope it. `preload` is never sent. |
| `RDRS_LOGIN_RATE_LIMIT_ATTEMPTS` | `5` | Attempts per client IP per window, per credential-endpoint class (password login, registration, passkey, password change; separate budgets). Password login also has a per-account budget of 4× this. Throttled → `429` with `Retry-After`. `0` disables. |
| `RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS` | `60` | Fixed window length in seconds (≥ 1). |
| `RDRS_USER_AGENT` | `RDRS/...` | User agent for feed fetching |
| `RDRS_FETCH_ALLOW_PRIVATE_HOSTS` | - | Comma-separated hostnames, IPs or CIDRs that outbound fetches (feeds, discovery, icons, full content, image proxy) may reach despite being non-public (e.g. `192.168.0.0/16, nas.local`). By default loopback, private, link-local, CGNAT (`100.64/10`, incl. Tailscale), multicast, reserved, and `.local`/`.internal` are refused, with redirects and DNS answers re-checked. A listed hostname is accepted wherever it resolves. Non-http(s) is always refused. |
| `RDRS_WEBAUTHN_RP_ID` | `localhost` | WebAuthn Relying Party ID |
| `RDRS_WEBAUTHN_RP_ORIGIN` | `http://localhost:{port}` | WebAuthn Relying Party origin |
| `RDRS_WEBAUTHN_RP_NAME` | `rdrs` | WebAuthn Relying Party display name |
| `RUST_LOG` | - | Log filter (e.g. `info`, `rdrs=debug`). Unset → `error,rdrs=info`. |
| `RDRS_LOG_FORMAT` | `full` | `full`, `compact`, `pretty`, or `json`. Also `--log-format`. |
| `RDRS_AUTH_PROXY_HEADER` | - | Forward-auth username header (e.g. `Remote-User`). Empty disables. |
| `RDRS_TRUSTED_PROXY_NETWORKS` | - | Comma-separated CIDRs/IPs (e.g. `10.0.0.0/8, 192.168.1.5`) the TCP peer must be in for identity headers to be trusted. Required with `RDRS_AUTH_PROXY_HEADER`. Also used to find the real client IP for rate limiting — set it whenever rdrs is behind a reverse proxy, or all clients share one bucket. |
| `RDRS_AUTH_PROXY_USER_CREATION` | `false` | `true` → create accounts for unknown proxy users instead of redirecting to `/login`. |
| `RDRS_AUTH_PROXY_GROUPS_HEADER` | - | Header with comma-separated groups (e.g. `Remote-Groups`). |
| `RDRS_AUTH_PROXY_ADMIN_GROUP` | - | Group granting admin, synced every forward-auth login. Requires `RDRS_AUTH_PROXY_GROUPS_HEADER`. |
| `RDRS_DISABLE_LOCAL_AUTH` | `false` | Hide the password form; `POST /api/session` → 403. GReader API and passkeys unaffected. Requires `RDRS_AUTH_PROXY_HEADER`. |
| `RDRS_AUTH_PROXY_LOGOUT_URL` | (unset) | Sign Out redirects here (e.g. Authelia logout) to end the SSO session. Unset → forward-auth Sign Out clears the local session and warns the user to log out at the proxy. |

> **Deploying behind a domain?** Set `RDRS_WEBAUTHN_RP_ID` and
> `RDRS_WEBAUTHN_RP_ORIGIN` to your public host (e.g. `rdrs.example.com`,
> `https://rdrs.example.com`) or passkeys fail. rdrs warns at startup while the
> origin is `localhost`; active values are on the Settings page.

### Structured Logging

Every log line has an `event` field (`domain.verb`: `feed.sync_failed`,
`retention.pruned`, `summary.worker_started`, `shutdown.signal`) and typed
value fields (`feed_id`, `entry_id`, `user_id`, `bucket`, `count`, `error`, …);
messages are static. Under `RDRS_LOG_FORMAT=json`:

```json
{"timestamp":"…","level":"WARN","target":"rdrs::services::feed_sync",
 "fields":{"message":"feed sync failed","event":"feed.sync_failed",
           "feed_id":42,"error":"connection timed out"}}
```

`target` is the module path, e.g. `RUST_LOG=rdrs::services::feed_sync=debug`.
`tests/logging_test.rs` enforces that every log call sets `event` and no
message interpolates values.

### Request Timing

`middleware/request_log.rs` logs each request once, when the response head is
ready, with `method`, `route`, `status`, `elapsed`, `elapsed_ms`:

| Event | Level | When |
| --- | --- | --- |
| `http.request` | DEBUG | Every request. Off under the default filter. |
| `http.slow_request` | WARN | The request took ≥ 1s. Visible by default, and carries `threshold_ms`. |

- Enable all: `RUST_LOG=rdrs=debug`; just this: `RUST_LOG=rdrs=info,rdrs::middleware::request_log=debug`.
- `route` is the matched template (`/invite/{token}`), never the raw path
  (tokens live in paths); unmatched requests log `<unmatched>`.
- Timing excludes body streaming (so `/events` SSE looks fast).
- DB side: `RUST_LOG=sqlx::query=debug` for every statement, `sqlx::query=warn`
  for those over 1 s.

### Audit Logging

Session creation/renewal/destruction, API-token issuance/revocation, failed
logins, rate-limited credential attempts, and masquerade start/stop are logged
under the `rdrs::audit` target. Isolate with `RUST_LOG=rdrs::audit=info` (or
`RUST_LOG=rdrs=info,rdrs::audit=info`); use `RDRS_LOG_FORMAT=json` for a SIEM.

Sessions are identified by `sid`: a salted HMAC-SHA256 of the token, truncated
to 16 hex chars. The salt is `RDRS_SECRET`, so rotating it (including restarts
without it) breaks `sid` correlation with older lines.

## Authentication & SSO

Local password, passkeys, and **forward-auth (trusted-header) SSO** all work
simultaneously; only `RDRS_DISABLE_LOCAL_AUTH` narrows this.

### Adding people

No registration form. The first account is created at `/setup`; afterwards an
admin adds accounts from `/admin`:

1. **Admin** enters a username and role. The account has no password yet.
2. **rdrs** shows a one-time link (`/invite/<token>`) once — only a keyed hash
   is stored, so copy it immediately.
3. **The new user** opens it, sets a password, and is sent to sign-in. Links
   are single-use and expire after **7 days**.

Issuing a new link revokes the previous one. On an account with a password it
acts as a **password reset** (there's no self-service recovery); the old
password works until the link is redeemed. Redeeming signs the account out
everywhere and revokes its GReader API tokens.

### Passwords

- **15–128 characters**, no composition rules; passphrases welcome. (NIST
  SP800-63B minimum for single-factor accounts.) Over-long passwords are
  rejected, not truncated.
- Also refused if [zxcvbn](https://github.com/dropbox/zxcvbn) rates them
  trivially guessable (e.g. `passwordpassword`, username plus digits); the error
  quotes zxcvbn's reason. Runs locally, nothing is sent anywhere.
- Existing passwords keep working; no forced rotation. Changing a password
  signs out other browser sessions **and** revokes all GReader API tokens.

### Admin actions ask for your password

Promoting, demoting, disabling, deleting, or viewing as another user requires
password confirmation within the last 5 minutes (a prompt appears on `/admin`).
Ending "view as" never asks. Forward-auth (SSO) sessions are exempt.

### Passkeys

"Login with Passkey" asks for no username, so passkeys must be discoverable;
new ones always are.

> **Passkeys from older releases** may no longer be offered (sign-in times out
> with "Authentication was cancelled or timed out.") on most security keys, some
> Windows Hello setups, and password managers like Bitwarden. iCloud Keychain and
> Google Password Manager are unaffected.
>
> Fix: sign in with your password, **delete the old passkey** in Settings
> first, then register a new one. Registering first fails because the old
> credential is excluded.

### Forward-Auth (SSO)

RDRS can delegate browser login to a forward-auth proxy (Authelia, authentik,
Traefik ForwardAuth), which passes the user's identity in a trusted header.
Existing accounts are matched by username.

**Required** (see [Configuration](#configuration)):
- `RDRS_AUTH_PROXY_HEADER` — username header (e.g. `Remote-User`).
- `RDRS_TRUSTED_PROXY_NETWORKS` — CIDR(s)/IP(s) the proxy connects from; the
  header is trusted only from these TCP peers.

**Optional:** `RDRS_AUTH_PROXY_USER_CREATION`, `RDRS_AUTH_PROXY_GROUPS_HEADER` +
`RDRS_AUTH_PROXY_ADMIN_GROUP`, `RDRS_DISABLE_LOCAL_AUTH`,
`RDRS_AUTH_PROXY_LOGOUT_URL` (without it, SSO users are told to log out at the
proxy, since the header would re-authenticate them).

**Reverse-proxy requirements:**

1. The proxy **must** set, and strip any client-supplied copy of, the identity
   and groups headers on every request. Otherwise the trust model is bypassed.
2. The proxy **must** bypass forward-auth for paths reached without a browser
   SSO session:
   - **GReader API** — `/accounts/ClientLogin`, `/reader/api/...`,
     `/api/greader.php/...` (authenticated by `ClientLogin` token).
   - **Image proxy** `/api/proxy/...` — `<img>` requests carry no SSO session,
     so gating it breaks images. Safe: every URL is HMAC-signed.
   - **`/health`**, if probes go through the proxy. It exposes status and build
     version unauthenticated; scope the rule to your monitor if that matters.
   - **PWA assets** — `/static/...`, `/favicon...`, `/apple-touch-icon.png`,
     `/sw.js`, `/offline` — for installability (the browser fetches manifest
     icons without the session). They hold no user data and RDRS already
     serves them sessionless (`SKIP_PREFIXES` in
     `src/middleware/forward_auth.rs`).

   Keep everything else behind SSO. Do **not** bypass `/api/feeds/{id}/icon`;
   it uses RDRS's own session check.

   Example Authelia rules:

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

> Internals: [ARCHITECTURE.md](ARCHITECTURE.md#forward-auth-trusted-header-login).

## Usage

### Adding Feeds

On the Feeds page, enter a feed URL or a webpage URL; RDRS auto-discovers the
feed.

### Keyboard Shortcuts

Vim-style navigation; press `?` for the full list.

### OPML Import/Export

Export or import feeds as OPML from Settings.

### AI Summaries

Per user, via the Kagi Universal Summarizer: Settings → Kagi Universal
Summarizer, paste your Kagi session link and choose a language. Summaries are
generated on demand.

### Linkding Integration

Settings → enter your Linkding URL and API token, then use "Save" on any entry.

### Install as an App

- **Desktop Chrome / Edge** — install icon in the address bar
- **Android Chrome** — ⋮ → *Install app*
- **iOS Safari** — Share → *Add to Home Screen*

Available once signed in (the sign-in page registers no service worker). Out of
the box only static assets and an offline page are cached — nothing of yours.
Requires HTTPS except on `http://localhost`; see
[Production Notes](#production-notes).

### Offline reading

Set **Keep offline** in *Settings → Preferences* to how many entries to keep
(`0`, the default, stores nothing). Newest unread entries are saved first,
starred ones fill the rest, with images.

- Syncs in the background; **/entries/offline** lists what's saved.
- Offline, the app opens that list and articles open normally.
- The dot beside the *rdrs* wordmark: steady green when connected; amber,
  captioned **Offline**, and breathing while retrying.
- Anything needing the server (mark read, star, Load More, search, full
  content, summaries) greys out. Nothing is queued.
- Saved entries are per account and deleted on sign-out, account switch, or
  setting the count to `0`.

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

Multi-stage build on a distroless base; see
[ARCHITECTURE.md](ARCHITECTURE.md#deployment).

### Production Notes

- Set a persistent `RDRS_SECRET` (otherwise restarts end sessions and break
  cached image-proxy URLs).
- Mount `/data` so the SQLite database persists.
- Terminate TLS at a reverse proxy.
- Forward `Host` exactly as the browser sent it, **port included**. Browsers
  without `Sec-Fetch-Site` (Safari < 16.4) are CSRF-checked by `Origin` vs
  `Host`, so nginx's `proxy_set_header Host $host` on a non-default port causes
  `403`s — use `$http_host`. Caddy and Traefik are fine by default.
- RDRS always sends `Content-Security-Policy`, `X-Content-Type-Options: nosniff`,
  `Referrer-Policy`, `Permissions-Policy`, `X-Frame-Options: DENY` and
  `Cross-Origin-Opener-Policy: same-origin`. Not configurable; a header your
  proxy already sets is never overwritten. HSTS is configured via `RDRS_HSTS*`.
- The CSP (`script-src 'self'`, `style-src 'self'`, no `'unsafe-inline'`,
  `img-src 'self'`) assumes `RDRS_PUBLIC_BASE_URL` is the origin browsers use;
  otherwise proxied article images are blocked.

## Development

### Prerequisites

- Rust (pinned in `rust-toolchain.toml`; rustup installs it)
- SQLite (bundled)

### Running Tests

```bash
cargo nextest run
```

### Project Structure

See [ARCHITECTURE.md](ARCHITECTURE.md).

## Tech Stack

- **Web Framework**: Axum 0.8
- **Async Runtime**: Tokio
- **Database**: SQLite / PostgreSQL (sqlx)
- **Templates**: Askama
- **Feed Parsing**: feed-rs
- **HTML Sanitization**: Ammonia
- **Content Extraction**: Readability

## License

MIT
