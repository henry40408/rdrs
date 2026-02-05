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
│   ├── favicon.rs       # Favicon serving
│   └── proxy.rs         # Image proxy
│
├── services/            # Business logic
│   ├── background.rs    # Background sync scheduler
│   ├── feed_sync.rs     # Feed refresh logic
│   ├── feed_discovery.rs# Feed URL detection
│   ├── readability.rs   # Content extraction
│   ├── sanitize.rs      # HTML sanitization
│   ├── opml.rs          # OPML import/export
│   ├── icon_fetcher.rs  # Feed icon fetching
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
│   └── flash.rs         # Flash messages
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
- Static file serving
- Cookie layer for sessions
- Database connection pool as state

### Configuration (`config.rs`)

Loads settings from environment variables:
- `DATABASE_URL` - SQLite file path
- `SERVER_PORT` - HTTP port
- `SIGNUP_ENABLED` / `MULTI_USER_ENABLED` - Registration settings
- `IMAGE_PROXY_SECRET` - HMAC secret for image proxy

### Error Handling (`error.rs`)

Custom `AppError` type that maps to appropriate HTTP responses:
- Authentication errors → 401
- Not found → 404
- Validation errors → 400
- Internal errors → 500

## Data Layer

### Database (`db/schema.rs`)

SQLite schema with 10 tables:

| Table | Purpose |
|-------|---------|
| `user` | User accounts with role (admin/user) |
| `session` | Session tokens with masquerade support |
| `category` | Feed categories per user |
| `feed` | Feed metadata with etag caching |
| `entry` | Feed items with read/starred status |
| `entry_summary` | AI-generated article summaries |
| `image` | Polymorphic image storage |
| `user_settings` | User preferences and service configs |
| `passkey` | WebAuthn credential storage |
| `webauthn_challenge` | WebAuthn challenge state |

### Models

Each model provides:
- Struct definition matching database schema
- CRUD operations as associated functions
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

## Services

### Feed Synchronization

**Background Scheduler** (`background.rs`):
- Runs continuously in a Tokio task
- Distributes feeds across 60-minute buckets based on ID hash
- Syncs feeds in the current bucket every minute

**Sync Logic** (`feed_sync.rs`):
- Uses etag/if-modified-since for efficient updates
- Parses feed with feed-rs library
- Inserts new entries, skips duplicates

### Content Processing

**HTML Sanitization** (`sanitize.rs`):
- Uses Ammonia for XSS protection
- Removes tracking parameters (utm_*, fbclid, etc.)
- Blocks tracking domains (pixel.*, analytics.*, etc.)
- Removes 1x1 tracking pixels
- Fixes relative image URLs

**Full Content Extraction** (`readability.rs`):
- Fetches article URL
- Extracts main content using readability algorithm
- Includes SSRF protection (blocks private IPs)

### Image Proxy (`image_proxy.rs`)

Proxies external images to:
- Protect user privacy (no direct requests to external servers)
- Work around mixed content issues

Uses HMAC-SHA256 signatures to prevent abuse:
1. Server generates signed URL: `/api/proxy/image?url=...&sig=...`
2. Proxy handler verifies signature before fetching

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

- 24-hour session expiry
- Secure cookie settings
- Masquerade feature for admin testing

### Input Sanitization

- All HTML content sanitized with Ammonia
- SQL injection prevented via parameterized queries
- SSRF protection in readability fetcher

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
