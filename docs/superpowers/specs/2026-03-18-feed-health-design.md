# Feed Health Design

## Overview

Enhance the existing Feeds page to display feed health information (last fetched time, last updated time, freshness status) and add a "Stale" filter option.

## Requirements

- **Scope**: Modify existing `/feeds` page, no new routes or pages
- **Architecture**: Pure SSR changes to handler + template
- **No new DB tables**: All data from existing `feed` table fields (`fetched_at`, `feed_updated_at`, `fetch_error`)

## Feed Row Enhancements

Each feed row currently shows: name, URL, category, unread count, error message.

Add to each feed row:

| Field | Source | Display |
|-------|--------|---------|
| Last fetched | `feed.fetched_at` | Relative time (e.g. "3 hours ago"), muted text. NULL → "Never" |
| Last updated | `feed.feed_updated_at` | Relative time with freshness color. NULL → "Never" in muted color |

## Freshness Status

Computed from `feed_updated_at` relative to current time. Fixed thresholds:

| Status | Condition | Visual |
|--------|-----------|--------|
| Fresh | `feed_updated_at` within 30 days | Normal text color (no special styling) |
| Warning | `feed_updated_at` between 30–90 days ago | Warning color (`--color-warning`) |
| Stale | `feed_updated_at` more than 90 days ago | Error color (`--color-error`) |
| Unknown | `feed_updated_at` is NULL | Muted color (`--color-text-muted`), display "Never" |

Freshness status is computed in the handler and passed to the template as an enum value per feed.

## Filter Enhancement

Current filters: `All` / `Errors only`

Add: `Stale` — shows feeds where `feed_updated_at` is NULL or more than 90 days ago.

Filters are mutually exclusive: All / Errors / Stale.

Implementation: query parameter `?filter=all|errors|stale`, default `all`. The handler filters the feed list server-side before rendering.

## Relative Time Formatting

Display timestamps as human-readable relative times:
- Under 1 hour: "X minutes ago"
- Under 24 hours: "X hours ago"
- Under 30 days: "X days ago"
- 30+ days: "X months ago"
- Over 365 days: "X years ago"

Computed server-side in the handler. The exact timestamp is available as a `title` attribute for hover tooltip.

## Data Model Changes

No schema changes. No new tables. All data from existing `feed` columns:
- `fetched_at TEXT` — last sync attempt time
- `feed_updated_at TEXT` — feed's most recent publication time
- `fetch_error TEXT` — error from last failed sync

### Handler Changes

Modify `FeedRow` struct in `src/handlers/pages.rs` to include:
- `fetched_at: Option<String>` — formatted relative time
- `fetched_at_datetime: Option<String>` — ISO datetime for tooltip
- `feed_updated_at: Option<String>` — formatted relative time
- `feed_updated_at_datetime: Option<String>` — ISO datetime for tooltip
- `freshness: FeedFreshness` — enum (Fresh / Warning / Stale / Unknown)

Add `FeedFreshness` enum and a helper function to compute it from `feed_updated_at`.

Add filter query parameter parsing to `feeds_page` handler.

### Template Changes

Modify `templates/feeds.html`:
- Add "Stale" filter link alongside existing "Errors only"
- Add last fetched / last updated display in each feed row
- Apply freshness CSS class to the last updated value

### CSS Changes

Add minimal CSS in `templates/base.html` for freshness classes:
- `.feed-freshness-warning` — uses `--color-warning`
- `.feed-freshness-stale` — uses `--color-error`

## Error Handling

- `fetched_at` NULL → display "Never" in muted color
- `feed_updated_at` NULL → display "Never" in muted color, freshness = Unknown
- Invalid filter parameter → default to "all"

## Testing

- **Handler integration tests**: feeds page renders with new fields, filter=errors works, filter=stale works, freshness classes present
- **Existing tests**: ensure no regressions on feeds page
