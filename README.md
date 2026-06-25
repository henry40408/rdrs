# RDRS - RSS Reader in Rust

[![CI](https://github.com/henry40408/rdrs/actions/workflows/ci.yml/badge.svg)](https://github.com/henry40408/rdrs/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/henry40408/rdrs/graph/badge.svg)](https://codecov.io/gh/henry40408/rdrs)
[![Release](https://img.shields.io/github/v/release/henry40408/rdrs)](https://github.com/henry40408/rdrs/releases/latest)
[![License](https://img.shields.io/github/license/henry40408/rdrs)](LICENSE.txt)
[![Rust toolchain](https://img.shields.io/badge/dynamic/toml?url=https://raw.githubusercontent.com/henry40408/rdrs/main/rust-toolchain.toml&query=$.toolchain.channel&label=rust%20toolchain&logo=rust)](https://www.rust-lang.org/)
[![Docker](https://img.shields.io/badge/docker-ghcr.io-blue.svg)](https://ghcr.io/henry40408/rdrs)
[![Casual Maintenance Intended](https://casuallymaintained.tech/badge.svg)](https://casuallymaintained.tech/)
[![Vibe Coded](https://img.shields.io/badge/vibe_coded-Claude-d97757?logo=anthropic&logoColor=white)](https://claude.com/claude-code)

A self-hosted RSS/Atom feed reader built with Rust. Privacy-focused, lightweight, and designed for personal use.

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
- **Docker Ready** - Single-binary deployment with all assets embedded, multi-platform container images

## Quick Start

### Using Docker (Recommended)

```bash
docker run -d \
  --name rdrs \
  -p 3000:3000 \
  -v rdrs_data:/data \
  -e SIGNUP_ENABLED=true \
  -e IMAGE_PROXY_SECRET="$(openssl rand -base64 32)" \
  ghcr.io/henry40408/rdrs:latest
```

Visit `http://localhost:3000` and create your account.

> **`IMAGE_PROXY_SECRET`** — if left unset, a random secret is generated on
> each startup, which invalidates every previously-proxied image URL whenever
> the container restarts. Set it to a persistent value (e.g.
> `openssl rand -base64 32`) so proxied images survive restarts.

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

Visit `http://localhost:3000` and create your account. The **first account is
always allowed** even when `SIGNUP_ENABLED=false` (the default), so a source
build works out of the box. `SIGNUP_ENABLED` (together with
`MULTI_USER_ENABLED`) only governs *additional* registrations after the first
account exists.

## Configuration

All configuration is done via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | `rdrs.sqlite3` | SQLite database file path |
| `SERVER_PORT` | `3000` | HTTP server port |
| `SIGNUP_ENABLED` | `false` | Allow new user registration |
| `MULTI_USER_ENABLED` | `false` | Allow multiple users (requires signup enabled) |
| `IMAGE_PROXY_SECRET` | Auto-generated | HMAC secret for secure image proxying |
| `PUBLIC_BASE_URL` | - | Public base URL for generating absolute image proxy URLs in API responses (e.g., `https://rdrs.example.com`). If not set, relative paths are used (backward compatible). |
| `USER_AGENT` | `RDRS/...` | Custom user agent for feed fetching |
| `WEBAUTHN_RP_ID` | `localhost` | WebAuthn Relying Party ID for passkey authentication |
| `WEBAUTHN_RP_ORIGIN` | `http://localhost:{port}` | WebAuthn Relying Party origin URL |
| `WEBAUTHN_RP_NAME` | `rdrs` | WebAuthn Relying Party display name |
| `RUST_LOG` | - | Log level filter (e.g., `info`, `debug`, `rdrs=debug`) |
| `AUTH_PROXY_HEADER` | - | Header carrying the username from a forward-auth proxy (e.g. `Remote-User`, `X-Forwarded-User`). Empty disables the feature. |
| `TRUSTED_PROXY_NETWORKS` | - | Comma-separated CIDRs or bare IPs (e.g. `10.0.0.0/8, 192.168.1.5`). The TCP peer IP must fall within one of these for the identity header to be trusted. Required when `AUTH_PROXY_HEADER` is set. |
| `AUTH_PROXY_USER_CREATION` | `false` | When `true`, JIT-create a local account for an unknown proxy-provided username instead of redirecting to `/login`. |
| `AUTH_PROXY_GROUPS_HEADER` | - | Header carrying comma-separated group names from the proxy (e.g. `Remote-Groups`). |
| `AUTH_PROXY_ADMIN_GROUP` | - | Membership in this group grants the admin role, synced on every forward-auth login. Active only when `AUTH_PROXY_GROUPS_HEADER` is also set. |
| `DISABLE_LOCAL_AUTH` | `false` | Hides the browser password form and rejects `POST /api/session` with 403. Does not affect GReader API or passkey auth. Requires `AUTH_PROXY_HEADER`. |

> **Deploying behind a domain?** `WEBAUTHN_RP_ID` and `WEBAUTHN_RP_ORIGIN`
> default to `localhost` and **must** be overridden to your public host (e.g.
> `WEBAUTHN_RP_ID=rdrs.example.com`,
> `WEBAUTHN_RP_ORIGIN=https://rdrs.example.com`), otherwise the browser rejects
> passkeys. rdrs logs a startup warning while the RP origin still points at
> `localhost`, and the active values are shown on the Settings page.

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

### Linkding Integration

Connect RDRS to your Linkding instance to save articles for later:

1. Go to Settings
2. Enter your Linkding URL and API token
3. Use the "Save" button on any entry

## Docker

### Docker Compose

```yaml
services:
  rdrs:
    image: ghcr.io/henry40408/rdrs:latest
    ports:
      - "3000:3000"
    volumes:
      - rdrs_data:/data
    environment:
      - SIGNUP_ENABLED=true
      - IMAGE_PROXY_SECRET=your-secret-here
    restart: unless-stopped

volumes:
  rdrs_data:
```

### Building Docker Image

```bash
docker build -t rdrs:latest .
```

The Dockerfile uses multi-stage builds with a distroless base image for minimal size and attack surface.

## Development

### Prerequisites

- Rust 1.96 (pinned via `rust-toolchain.toml`; rustup installs it automatically)
- SQLite (bundled via rusqlite)

### Running Tests

```bash
cargo test
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
