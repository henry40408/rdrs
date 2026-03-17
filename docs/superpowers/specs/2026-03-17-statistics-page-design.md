# Statistics Page Design

## Overview

Add a statistics dashboard to RDRS that gives users insight into their reading habits and feed activity. Admin users additionally see site-wide metrics.

## Requirements

- **Route**: `GET /statistics`
- **Architecture**: Pure SSR (Askama template), no client-side JS frameworks or chart libraries
- **Charts**: CSS-only bar charts and progress bars
- **Navigation**: New "Statistics" item in sidebar, same level as Unread/Starred/Search
- **Auth**: Requires login (authenticated route group)

## Period Selection

- Query parameters: `?period=7d|30d|90d|all|custom&from=YYYY-MM-DD&to=YYYY-MM-DD`
- Default period: `7d`
- Period selector aligned to the right of the page
- Fixed interval buttons: 7d, 30d, 90d, All
- Custom date range: two date inputs + Apply button, separated from fixed buttons by a divider
- Invalid custom range (from > to): fall back to default `7d`

## Personal Statistics (all users)

### Overview Cards (top row, grid)

| Metric | Source |
|--------|--------|
| Total Entries | `COUNT(entry)` where feed belongs to user's categories, within period |
| Read | `COUNT(entry) WHERE read_at IS NOT NULL`, within period |
| Unread | Total - Read |
| Read Rate | Read / Total as percentage |
| Starred | `COUNT(entry) WHERE starred_at IS NOT NULL`, within period |
| Summaries | `COUNT(entry_summary) WHERE status = 'completed'`, within period |

### Daily Read Articles (bar chart)

- CSS-only vertical bar chart
- X-axis: dates within the selected period
- Y-axis: count of entries marked as read on each date (based on `read_at`)
- Bar height calculated as percentage of max daily count

### Entries by Category (left column, progress bars)

- All categories for the user, sorted by entry count descending
- Each row: category name, count, horizontal progress bar (width relative to max)

### Top 10 Feeds (right column, progress bars)

- Top 10 feeds by entry count within the period
- Each row: feed title, count, horizontal progress bar (width relative to max)

## Admin Statistics (admin users only)

Rendered below personal stats, separated by a horizontal divider. Visually distinguished with different accent color.

### Admin Overview Cards

| Metric | Source |
|--------|--------|
| Total Users | `COUNT(user)` (not filtered by period) |
| Site Entries | `COUNT(entry)` across all users, within period |
| Total Feeds | `COUNT(feed)` across all users (not filtered by period) |
| Site Read Rate | site-wide read/total percentage, within period |

## Page Layout

```
┌──────────────────────────────────────────────────────┐
│                    [7d] [30d] [90d] [All] │ from—to │ (right-aligned)
├──────────────────────────────────────────────────────┤
│ [Total] [Read] [Unread] [Rate] [Starred] [Summaries]│ (overview cards grid)
├──────────────────────────────────────────────────────┤
│ Daily Read Articles                                  │
│ ▌ ██ ▌ ███ ▌ █ ▌ ████ ▌ ██ ▌ ██ ▌ █ ▌              │ (CSS bar chart)
├──────────────────────────────────────────────────────┤
│ Entries by Category    │ Top 10 Feeds                │ (two-column grid)
│ Tech ████████── 423    │ HN ██████████── 187         │
│ News █████──── 312     │ Ars ███████── 134            │
│ Blog ███──── 198       │ Verge █████── 98             │
├──────────────────────────────────────────────────────┤
│ Admin: Site-wide Statistics (admin only)              │
│ [Users] [Site Entries] [Total Feeds] [Site Rate]     │
└──────────────────────────────────────────────────────┘
```

## Data Model

No new database tables required. All statistics derived from existing tables via aggregate queries.

### New Query Functions

Add to `src/models/` (new file `statistics.rs` or extend existing model files):

- `get_personal_overview(user_id, from, to)` → totals, read count, starred, summaries
- `get_daily_read_counts(user_id, from, to)` → Vec<(date, count)>
- `get_entries_by_category(user_id, from, to)` → Vec<(category_name, count)>
- `get_top_feeds(user_id, from, to, limit)` → Vec<(feed_title, count)>
- `get_admin_overview(from, to)` → user count, site entries, feed count, site read rate

## Handler

- File: `src/handlers/pages.rs`
- Function: `statistics_page()`
- Parses `period` and `from`/`to` query params
- Computes date range from period
- Calls query functions
- Renders `templates/statistics.html`

## Template

- File: `templates/statistics.html`
- Extends base layout
- Uses existing CSS variables for theming (dark/light mode support)
- Admin section wrapped in `{% if is_admin %}`
- Empty states: display message when no data in period

## Sidebar

- Add "Statistics" link in `templates/macros.html`
- Positioned after Search, before Settings

## Error Handling

- Invalid `period` value → default to `7d`
- Invalid custom date range (from > to) → default to `7d`
- No data in period → show empty state text per section

## Testing

- **Model tests**: query functions with empty DB, populated DB, date boundary conditions
- **Handler integration tests**: page renders for normal user, admin sees extra section, period parameter parsing
- **E2E tests**: page accessible, period selector works, admin section visibility
