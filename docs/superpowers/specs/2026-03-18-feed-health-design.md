# Feed Health Design

## Overview

Enhance the existing Feeds page to display feed health information (last fetched time, last updated time, freshness status) and add a "Stale" filter option.

## Requirements

- **Scope**: Modify existing `/feeds` page, no new routes or pages
- **Architecture**: Pure SSR changes to handler + template
- **No new DB tables**: All data from existing `feed` table fields (`fetched_at`, `feed_updated_at`, `fetch_error`)

## Feed Row Enhancements

Each feed row currently shows: name, URL, category, unread count, error message.

Add health info as secondary text below the feed title within the existing Title `<td>` cell. This avoids adding new columns and keeps the mobile card layout (`mobile-cards`) intact.

| Field | Source | Display |
|-------|--------|---------|
| Last fetched | `feed.fetched_at` | Relative time (e.g. "3 hours ago"), muted text. NULL → "Never" |
| Last updated | `feed.feed_updated_at` | Relative time with freshness color. NULL → "Never" in muted color |

These fields are display-only and should NOT be added to `feed_data_json` (used for client-side edit/delete).

## Freshness Status

Computed from `feed_updated_at` relative to current time. Fixed thresholds:

| Status | Condition | Visual |
|--------|-----------|--------|
| Fresh | `feed_updated_at` within 30 days | Normal text color (no special styling) |
| Warning | `feed_updated_at` between 30–90 days ago | Warning color (`--color-warning`) |
| Stale | `feed_updated_at` more than 90 days ago | Error color (`--color-error`) |
| Unknown | `feed_updated_at` is NULL but `fetched_at` is recent (within 30 days) | Muted color (`--color-text-muted`), display "No date info" |
| Unknown + Stale | `feed_updated_at` is NULL and `fetched_at` is NULL or old (>90 days) | Error color, display "Never" — treated as stale for filtering |

Note: Some feeds are fetched successfully but have no publication dates in their entries. If `fetched_at` is recent, this is not a health problem — the feed just lacks date metadata.

Freshness is computed in the handler and passed to the template as a pre-computed CSS class string (e.g. `""`, `"feed-freshness-warning"`, `"feed-freshness-stale"`) to avoid enum matching in the template. This follows the existing pattern of passing pre-computed display values.

## Filter Enhancement

Current filters: `All` / `Errors only` — implemented **client-side** via JavaScript (`handleFilterChange()` toggling row visibility).

Add: `Stale` — shows feeds where freshness is Stale or Unknown+Stale.

Filters remain **client-side** for consistency with the existing implementation. The handler adds a `data-freshness` attribute (e.g. `data-freshness="fresh"`, `"warning"`, `"stale"`) to each feed row `<tr>`. The existing JS filter logic is extended to support the new "Stale" option by checking this attribute.

Filters are mutually exclusive: All / Errors / Stale.

## Relative Time Formatting

Display timestamps as human-readable relative times:
- Under 1 minute: "Just now"
- Under 1 hour: "X minutes ago"
- Under 24 hours: "X hours ago"
- Under 30 days: "X days ago"
- Under 365 days: "X months ago"
- Over 365 days: "X years ago"

Computed server-side in the handler as a helper function. The exact ISO timestamp is available as a `title` attribute for hover tooltip.

## Data Model Changes

No schema changes. No new tables. All data from existing `feed` columns:
- `fetched_at TEXT` — last sync attempt time
- `feed_updated_at TEXT` — feed's most recent publication time
- `fetch_error TEXT` — error from last failed sync

### Handler Changes

Modify `FeedRow` struct in `src/handlers/pages.rs` to include:
- `fetched_at_relative: String` — formatted relative time (or "Never")
- `fetched_at_datetime: String` — ISO datetime for tooltip (or empty)
- `feed_updated_at_relative: String` — formatted relative time (or "Never" / "No date info")
- `feed_updated_at_datetime: String` — ISO datetime for tooltip (or empty)
- `freshness_class: String` — pre-computed CSS class (`""` / `"feed-freshness-warning"` / `"feed-freshness-stale"`)
- `freshness_value: String` — data attribute value (`"fresh"` / `"warning"` / `"stale"`) for client-side filtering

Add helper functions:
- `format_relative_time(dt: Option<DateTime<Utc>>) -> (String, String)` — returns (relative_text, iso_datetime)
- `compute_freshness(feed_updated_at: Option<DateTime<Utc>>, fetched_at: Option<DateTime<Utc>>) -> (String, String)` — returns (css_class, data_value)

### Template Changes

Modify `templates/feeds.html`:
- Add "Stale" filter button alongside existing "Errors only"
- Add health info (last fetched / last updated) as secondary text below feed title in each row
- Add `data-freshness` attribute to each feed `<tr>`
- Apply freshness CSS class to the last updated value
- Extend `handleFilterChange()` JS to support "stale" filter

### CSS Changes

Add minimal CSS in `templates/base.html` for freshness classes:
- `.feed-freshness-warning` — uses `--color-warning`
- `.feed-freshness-stale` — uses `--color-error`
- `.feed-health-info` — secondary text styling (smaller font, muted, below title)

## Error Handling

- `fetched_at` NULL → display "Never" in muted color
- `feed_updated_at` NULL + `fetched_at` recent → display "No date info" in muted color (not a health issue)
- `feed_updated_at` NULL + `fetched_at` NULL or old → display "Never" in error color (treated as stale)
- Invalid filter value from JS → no-op (show all)

## Testing

- **Handler integration tests**: feeds page renders with new fields, `data-freshness` attributes present, health info displayed
- **Existing tests**: ensure no regressions on feeds page
