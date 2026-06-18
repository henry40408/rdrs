# Interactive Daily Read Articles Chart — Design

Date: 2026-06-18
Status: Approved (pending spec review)

## Problem

The "Daily Read Articles" chart on `/statistics` is a pure-CSS bar chart that
renders shape only — it shows no numbers and has no interactivity. Exact values
are reachable solely through the browser-native `title` tooltip on hover, which
is invisible on touch screens. We want an interactive chart that surfaces the
numbers and works on touch devices.

## Goals

- Always-visible numeric Y-axis scale (no longer "shape only").
- Tap/click a bar to highlight it and show an info card with that day's value.
- First-class touch support (Pointer Events; no double-tap-zoom interference).
- Stay within project constraints: SSR-first, no frontend build tooling, vanilla
  ES modules embedded via `include_str!`, no charting library.

## Non-Goals

- No line/area chart, no drag-scrub. (Considered and rejected during design:
  user chose "C. bars + tap-to-focus" and "tap single bar only".)
- No trend/delta comparison in the info card (date + count only).
- No changes to the data query or the other two charts on the page.

## Chosen Direction (from visual brainstorming)

- **Chart type:** bars with a low-key base color; tapping a bar promotes it to the
  accent color and reveals an info card. Y-axis scale + gridlines always visible.
- **Interaction:** tap a single bar to show; tap another to switch; tap empty
  space / the same bar again / `Esc` to dismiss.
- **Info card contents:** `MM/DD · count` (date + count).

## Architecture

Keep the existing server-rendered `div`-bar model (consistent with the page's
other `.stats-progress` charts). No SVG rewrite. Changes are localized:

| Layer | File | Change |
|-------|------|--------|
| Handler | `src/handlers/pages/mod.rs` | Compute `y_ticks` (nice-max tick values) alongside existing `daily_read_counts` / `daily_max`; add to `StatisticsTemplate`. |
| View model | `src/handlers/pages/mod.rs` | Add `y_ticks: Vec<i64>` (or a small tick struct with value + position%) to the template struct. |
| Template | `templates/statistics.html` | Render Y-axis scale labels + gridlines; add `data-date` / `data-count` and `tabindex`/`role` to each bar column; wrap chart in `<reading-chart>`; keep `title` as no-JS fallback. |
| CSS | `static/css/app.css` | Styles for axis labels, gridlines, `.stats-bar` highlighted state, info card, `touch-action: manipulation`. |
| JS | `static/js/reading-chart.js` (new) | Custom element `<reading-chart>` using Pointer Events for tap highlight + info card. |
| JS registration | wherever chrome custom elements are registered / served | Register/serve the new module via the existing static allowlist + `include_str!` pattern. |

### Y-axis ticks (nice-max)

Handler computes a "nice" maximum ≥ `daily_max` and a small set of evenly spaced
tick values (e.g. 0, ¼, ½, ¾, max). Each bar's existing `height_percent` stays
relative to the same nice-max so bars and gridlines align. Edge cases:

- `daily_max == 0` → keep the existing "No read activity" message; no ticks.
- single non-zero day → ticks still produce a sane scale (e.g. max rounded up).

### `<reading-chart>` custom element (vanilla)

- Attaches `pointerdown` listeners (covers mouse / touch / pen) to bar columns.
- On activate: clear any previously highlighted bar, add highlighted class to the
  target, position an info card (absolutely positioned within the chart) above the
  bar showing `MM/DD · count` read from `data-*`.
- Dismiss on: tapping empty chart area, re-tapping the active bar, or `Esc`.
- Keyboard: bars are focusable; `Enter`/`Space` activates the focused bar.
- `touch-action: manipulation` on the chart to suppress double-tap zoom.
- Progressive enhancement: with JS disabled, bars + axis + native `title` still
  render and are readable.

## Data Flow

```
get_daily_read_counts (unchanged SQL)
  -> daily_max
  -> nice_max + y_ticks            (new, handler-side)
  -> DailyReadView { date, count, height_percent, short_label }  (unchanged)
  -> StatisticsTemplate { ..., y_ticks }
  -> Askama renders bars (+ data-*) + axis + gridlines
  -> <reading-chart> JS enhances tap interaction
```

## Error / Edge Handling

- No data in period: existing empty-state message; chart + JS not rendered.
- JS fails to load / disabled: static chart with axis numbers + native `title`.
- Very many days (wide range): bars already flex to fit; tap targets remain the
  full column width, not just the bar, so thin bars stay tappable.

## Testing

- **Unit (Rust):** `y_ticks` / nice-max computation — typical range, empty data,
  single-day, and a max that is already "nice".
- **E2E (Playwright BDD):** tap a bar → info card appears with the correct
  `MM/DD · count`; tap empty area → dismissed. Use pointer/touch interaction.
- **Screenshots:** UI changed, so rebuild (`cargo build`) and regenerate the four
  `screenshots/` images referenced by `README.md`.

## Constraints Recap

- No bundler/transpiler; vanilla ES module served via `include_str!`.
- `cargo fmt` + `cargo clippy -D warnings` must pass; tests via `cargo nextest`.
- Rebuild before E2E/screenshots so embedded assets are fresh.
