# Summarizer: ad-hoc URL summaries

**Date:** 2026-07-09
**Status:** Approved (design)

## Problem

Kagi summaries today are only reachable through a feed **entry** — the reading
pane's Summarize button queues a job keyed by `(user_id, entry_id)` and persists
the result in `entry_summary`. There is no way to summarize an arbitrary URL that
isn't already in your library. Users want a scratchpad: paste one or more links,
get a Kagi summary for each, without subscribing to or saving anything.

## Goals

1. A logged-in-only **Summarizer** page: a textarea of URLs (one per line) that
   produces a Kagi summary per URL, rendered in the existing `summary-box` format.
2. Summaries resolve **strictly in order**, top to bottom, with live progress —
   the running URL shows *Summarizing…*, the rest show *Queued*, each swaps in
   place as it completes.
3. Reuse the user's existing Kagi session and target-language from Settings.

## Non-Goals

- **No persistence.** Results are ephemeral; nothing is written to
  `entry_summary` or any new table. No migration.
- **No worker/queue.** The existing MPSC summary worker is entry-scoped and
  stays untouched. Sequencing is driven client-side (see Architecture).
- **No parallelism**, no "summarize all in background", no history of past runs.
- No new sidebar counts or badges.

## Behavioural Decisions

| Item | Decision |
|---|---|
| Placement | New **Tools** group in the sidebar, above Search. Route `GET /summarizer`. |
| Access | Logged-in only. If Kagi is not configured, the page shows a prompt linking to Settings instead of the form. |
| Input | One URL per line, trimmed; blank lines dropped; **max 30** per run. |
| Ordering | Strictly sequential, DOM order = input order. One in flight at a time. |
| Card title | Kagi's returned `Title:` line; fall back to the URL host when absent. |
| Card link | The submitted URL. |
| Language | The user's Kagi target-language setting (same as entry summaries). |
| Error | Per-card error banner (reuses `.summary-error-banner`) with **Retry** + **Dismiss**. A failed URL does not stop the run — the next one still starts. |
| Cancel | The in-flight card has **Cancel**; cancelling aborts that request and stops the remaining queue. |
| Timeout | Relies on the Kagi HTTP client's `EXTERNAL_API_TIMEOUT` per request. |

## Architecture

SSR-first with the existing `data-swap` progressive-enhancement helper. The
server owns validation and rendering; a small page-scoped ES module drives the
one-at-a-time sequencing (a single long streamed request is rejected — 30×timeout
would blow the app's Timeout layer, so each URL is its own short request).

### Routes (`src/lib.rs`)

- `GET  /summarizer` → `pages::summarizer_page` — renders `summarizer.html`
  (app layout + sidebar). Shows the not-configured prompt when Kagi is unset.
- `POST /summarizer` → `handlers::summarizer::start` — parses the textarea,
  trims/dedupes/validates (SSRF via `utils::url_validation::validate_url`, cap
  30). On error re-renders the form with the message. On success renders the
  results scaffold: the repopulated form plus one **Queued** `summary-box` card
  per validated URL, each tagged `data-summarizer-url` + index.
- `POST /summarizer/item` → `handlers::summarizer::item` — body: one `url`.
  Loads the user's `KagiConfig`, re-validates the URL, calls
  `kagi::summarize_url`, and returns a single card fragment (completed or error)
  as a `<template data-swap-target="#sz-card-{i}">`.

### Kagi service (`src/services/summarize/kagi.rs`)

`summarize_url` already strips the `Title: …\n\n` prefix from Kagi's markdown and
puts the body in `output_text`. Add `title: Option<String>` to `SummarizeResult`,
populated with that stripped title (else `None`). The entry-summary worker keeps
reading `output_text` only — additive, backward-compatible.

### Frontend

- `templates/summarizer.html` — page: intro, the not-configured prompt or the
  form, an empty `#sz-results` region, and the page module `<script>`.
- `templates/_summarizer_card.html` — one card, parameterised by state
  (`queued` | `summarizing` | `completed` | `error`), reused by both `/summarizer`
  (queued scaffold) and `/summarizer/item` (resolved fragment). Matches the
  entry-summary `summary-box` markup exactly.
- `static/js/pages/summarizer.js` — page-scoped module. After the `/summarizer`
  POST swaps in the queued cards, it walks `#sz-results` in order: mark the next
  card *Summarizing…*, `fetch` `POST /summarizer/item` with the URL, swap in the
  returned fragment, advance. **Cancel** aborts the in-flight `fetch`
  (`AbortController`) and halts the walk; **Retry** re-runs a single failed card.
  Reuses the shared `swap()` template-swap conventions; no new global wiring.

### Copy / Dismiss

Completed cards reuse the existing **Copy** (`data-summary-copy`) behaviour.
**Dismiss** removes the card from the DOM only (nothing persisted to delete).

## Testing

- **Rust unit** (`kagi.rs`): `title` is populated from the `Title:` prefix and is
  `None` when absent; `output_text` body is unchanged from today.
- **Rust handler**: `POST /summarizer` rejects >30 URLs and malformed/blocked
  URLs with a form error; drops blank lines; the not-configured page renders the
  Settings prompt. `POST /summarizer/item` returns a completed card for a stubbed
  Kagi success and an error card for a Kagi failure. Uses the existing
  `KAGI_API_BASE` stub.
- **E2E** (`e2e/`): a feature seeding a Kagi stub — submit 2–3 URLs, assert cards
  resolve in order and land completed/error states; assert the not-configured
  prompt when Kagi is unset. Tag network-flaky bits `@skip` if needed.

## Screenshots

Adding the **Summarizer** nav item changes the sidebar, which is visible in the
existing README screenshots (unread list with reading pane). After building,
regenerate all four via `cargo build && cd e2e && npm run screenshots`.
