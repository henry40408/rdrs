# RDRS - RSS Reader in Rust

[![CI](https://github.com/henry40408/rdrs/actions/workflows/ci.yml/badge.svg)](https://github.com/henry40408/rdrs/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/henry40408/rdrs/graph/badge.svg)](https://codecov.io/gh/henry40408/rdrs)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE.txt)
[![Rust](https://img.shields.io/badge/rust-1.92%2B-blue.svg)](https://www.rust-lang.org/)
[![Docker](https://img.shields.io/badge/docker-ghcr.io-blue.svg)](https://ghcr.io/henry40408/rdrs)
[![Casual Maintenance Intended](https://casuallymaintained.tech/badge.svg)](https://casuallymaintained.tech/)

A self-hosted RSS/Atom feed reader built with Rust. Privacy-focused, lightweight, and designed for personal use.

## Features

- **Feed Management** - Subscribe to RSS/Atom feeds, organize into categories, OPML import/export
- **Reading Experience** - Mark read/unread, star entries, full-text search, keyboard shortcuts
- **Privacy Protection** - HTML sanitization, tracking URL removal, image proxy
- **Full Content Extraction** - Fetch complete article content using readability algorithm
- **External Services** - Save entries to Linkding bookmark manager
- **Multi-User Support** - Role-based access control with admin panel
- **Docker Ready** - Multi-platform container images with minimal footprint

## Quick Start

### Using Docker (Recommended)

```bash
docker run -d \
  --name rdrs \
  -p 3000:3000 \
  -v rdrs_data:/data \
  -e SIGNUP_ENABLED=true \
  ghcr.io/henry40408/rdrs:latest
```

Visit `http://localhost:3000` and create your account.

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

## Configuration

All configuration is done via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | `rdrs.sqlite3` | SQLite database file path |
| `SERVER_PORT` | `3000` | HTTP server port |
| `SIGNUP_ENABLED` | `false` | Allow new user registration |
| `MULTI_USER_ENABLED` | `false` | Allow multiple users (requires signup enabled) |
| `IMAGE_PROXY_SECRET` | Auto-generated | HMAC secret for secure image proxying |
| `USER_AGENT` | `RDRS/...` | Custom user agent for feed fetching |

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

1. Go to User Settings
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

- Rust 1.92+
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
