# rdrs

> A RSS reader web app

## Tech Stack

### Backend

- **Framework**: Axum
- **Database**: rusqlite (SQLite)
- **Templating**: Askama
- **Authentication**: Cookie-based session

### Frontend

- Static HTML, CSS
- Minimal JavaScript (only when necessary)

## Data Models

### User

| Field         | Type     | Constraints       |
| ------------- | -------- | ----------------- |
| id            | INTEGER  | PRIMARY KEY       |
| username      | TEXT     | NOT NULL          |
| password_hash | TEXT     | NOT NULL          |
| created_at    | DATETIME | NOT NULL, DEFAULT |

- **Unique**: (username)

### Session

| Field         | Type     | Constraints        |
| ------------- | -------- | ------------------ |
| id            | INTEGER  | PRIMARY KEY        |
| user_id       | INTEGER  | NOT NULL, FK(User) |
| session_token | TEXT     | NOT NULL           |
| created_at    | DATETIME | NOT NULL, DEFAULT  |
| expires_at    | DATETIME | NOT NULL           |

### Category

| Field      | Type     | Constraints        |
| ---------- | -------- | ------------------ |
| id         | INTEGER  | PRIMARY KEY        |
| user_id    | INTEGER  | NOT NULL, FK(User) |
| name       | TEXT     | NOT NULL           |
| updated_at | DATETIME | NOT NULL, DEFAULT  |

- **Unique**: (user_id, name)

### Feed

| Field            | Type     | Constraints            |
| ---------------- | -------- | ---------------------- |
| id               | INTEGER  | PRIMARY KEY            |
| category_id      | INTEGER  | NOT NULL, FK(Category) |
| url              | TEXT     | NOT NULL               |
| title            | TEXT     | NOT NULL               |
| description      | TEXT     |                        |
| last_fetched_at  | DATETIME |                        |
| last_fetch_error | TEXT     |                        |
| feed_updated_at  | DATETIME |                        |
| etag             | TEXT     |                        |
| last_modified    | TEXT     |                        |
| updated_at       | DATETIME | NOT NULL, DEFAULT      |

- **Unique**: (category_id, url)

### Article

| Field        | Type     | Constraints        |
| ------------ | -------- | ------------------ |
| id           | INTEGER  | PRIMARY KEY        |
| feed_id      | INTEGER  | NOT NULL, FK(Feed) |
| url          | TEXT     | NOT NULL           |
| title        | TEXT     | NOT NULL           |
| content      | TEXT     |                    |
| published_at | DATETIME |                    |
| fetched_at   | DATETIME | NOT NULL           |

- **Unique**: (feed_id, url)

### ArticleStatus

| Field      | Type     | Constraints           |
| ---------- | -------- | --------------------- |
| id         | INTEGER  | PRIMARY KEY           |
| article_id | INTEGER  | NOT NULL, FK(Article) |
| read_at    | DATETIME |                       |
| starred_at | DATETIME |                       |

- **Unique**: (article_id)

### Image

| Field         | Type     | Constraints       |
| ------------- | -------- | ----------------- |
| id            | INTEGER  | PRIMARY KEY       |
| object_type   | TEXT     | NOT NULL          |
| object_id     | INTEGER  | NOT NULL          |
| data          | BLOB     | NOT NULL          |
| mime_type     | TEXT     | NOT NULL          |
| etag          | TEXT     |                   |
| last_modified | TEXT     |                   |
| created_at    | DATETIME | NOT NULL, DEFAULT |

- **Unique**: (object_type, object_id)
- **object_type values**: `feed` (feed icon)

**Usage**:
- Feed icon: `object_type = 'feed'`, `object_id = feed.id`

## Pages (HTML Routes)

| Method | Path              | Description          | Auth |
| ------ | ----------------- | -------------------- | ---- |
| GET    | `/`               | Home / Article list  | Yes  |
| GET    | `/login`          | Login page           | No   |
| GET    | `/register`       | Registration page    | No   |
| GET    | `/categories`     | Category management  | Yes  |
| GET    | `/categories/:id` | Articles in category | Yes  |
| GET    | `/feeds/:id`      | Articles in feed     | Yes  |
| GET    | `/articles/:id`   | Article detail       | Yes  |
| GET    | `/starred`        | Starred articles     | Yes  |
| GET    | `/settings`       | User settings        | Yes  |
| GET    | `/search`         | Search results page  | Yes  |

## API Endpoints

### Authentication

| Method | Path            | Description            | Auth |
| ------ | --------------- | ---------------------- | ---- |
| POST   | `/api/register` | Create new user        | No   |
| POST   | `/api/login`    | Login, create session  | No   |
| POST   | `/api/logout`   | Logout, delete session | Yes  |

### Categories

| Method | Path                  | Description     | Auth |
| ------ | --------------------- | --------------- | ---- |
| GET    | `/api/categories`     | List categories | Yes  |
| POST   | `/api/categories`     | Create category | Yes  |
| PUT    | `/api/categories/:id` | Update category | Yes  |
| DELETE | `/api/categories/:id` | Delete category | Yes  |

### Feeds

| Method | Path                   | Description        | Auth |
| ------ | ---------------------- | ------------------ | ---- |
| GET    | `/api/feeds`           | List feeds         | Yes  |
| POST   | `/api/feeds`           | Subscribe to feed  | Yes  |
| PUT    | `/api/feeds/:id`       | Update feed        | Yes  |
| DELETE | `/api/feeds/:id`       | Unsubscribe feed   | Yes  |
| POST   | `/api/feeds/:id/fetch` | Force refresh feed | Yes  |

**Feed Discovery (`POST /api/feeds`)**:

User submits only a URL. The server determines content type and handles accordingly:

1. **URL points to a feed (RSS/Atom/JSON Feed)**:
   - Detect via `Content-Type` header or XML/JSON parsing
   - Extract metadata from feed: `title`, `description`, `link`

2. **URL points to a webpage (HTML)**:
   - Parse HTML and look for feed links in `<link>` tags:
     ```html
     <link rel="alternate" type="application/rss+xml" href="..." />
     <link rel="alternate" type="application/atom+xml" href="..." />
     <link rel="alternate" type="application/feed+json" href="..." />
     ```
   - If multiple feeds found, prefer the first one (or let user choose)
   - Fetch the discovered feed URL
   - Merge metadata: webpage `<title>` and `<meta name="description">` as fallback if feed metadata is missing

3. **No feed found**:
   - Return error: "No feed found at this URL"

**Request Body**:
```json
{
  "url": "https://example.com",
  "category_id": 1
}
```

**Response** (success):
```json
{
  "id": 42,
  "url": "https://example.com/feed.xml",
  "title": "Example Blog",
  "description": "A blog about examples"
}
```

### Articles

| Method | Path                     | Description               | Auth |
| ------ | ------------------------ | ------------------------- | ---- |
| GET    | `/api/articles`          | List articles (paginated) | Yes  |
| GET    | `/api/articles/:id`      | Get article detail        | Yes  |
| POST   | `/api/articles/:id/read` | Mark as read              | Yes  |
| DELETE | `/api/articles/:id/read` | Mark as unread            | Yes  |
| POST   | `/api/articles/:id/star` | Star article              | Yes  |
| DELETE | `/api/articles/:id/star` | Unstar article            | Yes  |

### User

| Method | Path                 | Description          | Auth |
| ------ | -------------------- | -------------------- | ---- |
| GET    | `/api/user`          | Get current user     | Yes  |
| PUT    | `/api/user`          | Update user settings | Yes  |
| PUT    | `/api/user/password` | Change password      | Yes  |

### OPML

| Method | Path               | Description            | Auth |
| ------ | ------------------ | ---------------------- | ---- |
| GET    | `/api/opml/export` | Export feeds as OPML   | Yes  |
| POST   | `/api/opml/import` | Import feeds from OPML | Yes  |

### Search

| Method | Path          | Description     | Auth |
| ------ | ------------- | --------------- | ---- |
| GET    | `/api/search` | Search articles | Yes  |

## Background Tasks

### Feed Fetcher

Feeds are distributed into 60 buckets. One bucket is updated per minute, ensuring all feeds are refreshed within an hour.

**Bucket Assignment Algorithm**:

Derive bucket from feed URL using FNV-1a hash:

```
bucket = fnv1a_hash(feed_url) % 60
```

FNV-1a (32-bit) implementation:
```rust
fn fnv1a_hash(s: &str) -> u32 {
    const FNV_OFFSET: u32 = 2166136261;
    const FNV_PRIME: u32 = 16777619;

    s.bytes().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ byte as u32).wrapping_mul(FNV_PRIME)
    })
}
```

**Execution Flow**:

1. Triggered every minute (cron: `* * * * *`)
2. Calculate current bucket: `current_minute % 60`
3. Query all feeds: `SELECT * FROM feeds`
4. Filter feeds where `fnv1a_hash(url) % 60 == current_bucket`
5. Fetch all feeds in the bucket concurrently

**Fetch Behavior**:

- Respect `ETag` and `Last-Modified` headers for conditional requests
- Handle fetch errors gracefully, store error message in `last_fetch_error`
- Update `last_fetched_at` on each attempt
- Update `feed_updated_at` when new content is found

## Query Parameters

### Feed List (`/api/feeds`)

| Param       | Type    | Description        |
| ----------- | ------- | ------------------ |
| category_id | integer | Filter by category |

### Article List (`/api/articles`)

| Param       | Type    | Description                            |
| ----------- | ------- | -------------------------------------- |
| category_id | integer | Filter by category                     |
| feed_id     | integer | Filter by feed                         |
| unread_only | boolean | Show only unread articles              |
| starred     | boolean | Show only starred articles             |
| page        | integer | Page number (default: 1)               |
| per_page    | integer | Items per page (default: 20, max: 100) |

### Search (`/api/search`)

| Param       | Type    | Description                            |
| ----------- | ------- | -------------------------------------- |
| q           | string  | Search query (required)                |
| category_id | integer | Filter by category                     |
| feed_id     | integer | Filter by feed                         |
| page        | integer | Page number (default: 1)               |
| per_page    | integer | Items per page (default: 20, max: 100) |

**Search Behavior**:

- Search scope: article title (`title`) and content (`content`)
- Uses case-insensitive `LIKE` queries
- Query: `WHERE title LIKE '%query%' OR content LIKE '%query%' COLLATE NOCASE`

## Security

- Passwords hashed with Argon2
- Session tokens are cryptographically random
- CSRF protection for state-changing operations
- Rate limiting on authentication endpoints

# Appendix

- Always use English in this document.
