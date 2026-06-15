# Reading-Pane Image Placeholders, Loading Skeleton & Dimension Reservation

**Date:** 2026-06-16
**Status:** Approved (design)

## Problem

Images in reading-pane entry content load with no reserved space, no loading
affordance, and no failure handling:

- **No space reservation → layout shift (CLS).** A DB analysis of real content
  (1161 entries, 571 with images, 2999 `<img>` total) shows **60% of images
  carry no `width`/`height`** (1799/2999). With the current
  `.reading-pane-article img { height: auto }` rule these images reserve zero
  height until they load, so content collapses then jumps. Every image is
  routed through the HMAC image proxy (a re-fetch from origin), making the gap
  long and the jump obvious. `loading="lazy"` compounds it while scrolling.
- **No loading affordance.** Nothing is shown while an image loads — just blank.
- **No failure handling.** A proxy/404/SSRF failure shows the browser's default
  broken-image glyph.

The 39% of images that *do* carry `width`/`height` already reserve space
correctly (browsers derive `aspect-ratio` from the attributes when
`height: auto`), so the work targets the dimensionless 60%.

### Data findings that shape the design

- 60% no dims, 39% both dims, 0.9% one dim.
- `srcset`: 12.4% (must stay stripped by ammonia so proxy isn't bypassed).
- `src="data:"` permanent placeholder: 0.4% — out of scope.
- Unhandled `data-*`: 8.5%, but mostly `data-original-width`/`data-original-height`
  (dimension *hints*, not srcs) — a cheap, zero-network win.
- imgs/entry: median 2, mean 5.3, max 53 — dimension fetching must be bounded
  and off the render path.

## Goals

1. Reserve space for content images so the reading pane stops collapsing/jumping.
2. Show a loading placeholder (skeleton) and a graceful broken-image fallback.
3. Progressively reach zero-CLS for dimensionless images by measuring and
   caching their intrinsic size.

## Non-Goals

- Permanent `data:` placeholder recovery (problem B) — 0.4%, not worth it.
- Resizing/optimizing images (the proxy stays a passthrough).
- Persisting measured dimensions across restarts (in-memory cache only — see
  "Layer 3").
- List-view / favicon images (favicons are handled separately).

## Visual Decisions (reviewed in companion)

- **Loading skeleton:** static tinted block — `background: var(--color-bg-secondary)`,
  centered muted image glyph. No animation (chosen for restraint/perf).
- **Broken-image fallback:** dashed-border box (`1px dashed var(--color-border-light)`)
  on the same tint, centered broken-image glyph + the `alt` text as a muted
  caption (`var(--font-ui)`, `--color-text-muted`). Neutral palette, **not** the
  error red — a failed content image is not a user error. With no `alt`: glyph +
  "Image unavailable". The box keeps the already-reserved height (no collapse).

## Architecture

A layered, progressive-enhancement design. Server-rendered HTML carries
dimensions when known; vanilla JS provides the skeleton/error visuals; a
background worker fills in missing dimensions over time.

```
sanitize img_handler (per <img>, render time)
  1. has width+height (39%)          → keep as-is                  [Layer 1]
  2. else harvest dimension hints     → inject width/height if found [hints]
       (data-original-width/height, srcset descriptors, style w/h)
  3. else look up the in-memory dimension cache by ORIGINAL url
       • hit  → inject width/height                                [Layer 3]
       • miss → collect url for the builder to enqueue; leave undimensioned
  4. rewrite src → proxy, set loading=lazy, decoding=async,
     tag with data-img-state="loading"
  (srcset must remain stripped by ammonia → never bypasses the proxy)
```

### Layer 1 — preserve source dimensions (free)

Already works for 39%. Confirm ammonia keeps `width`/`height` on `img` (its
default `img` attributes include them — the existing tracking-pixel size check
in `rewrite_post_ammonia` relies on this). Add no code beyond a test asserting
dimensions survive sanitization. The existing `.reading-pane-article img`
`max-width:100%; height:auto` already yields aspect-ratio reservation when both
attributes are present.

### Dimension-hint harvest (cheap, zero-network)

In `sanitize`'s `img_handler`, before the proxy rewrite, when an image lacks
`width`/`height`, derive them from, in priority order:
1. `data-original-width` + `data-original-height` (seen in real data),
2. the largest `srcset`/`data-srcset` candidate's width descriptor (width only —
   no height, so skip unless a paired height exists; conservative: skip),
3. `style="width:..px;height:..px"` inline dimensions.
Inject any pair found as `width`/`height` attributes. Drop the now-redundant
`data-*` hint attributes (ammonia already strips them, so just read them in the
pre-ammonia lol_html pass or read from the source before ammonia — see
"Implementation notes").

### Layer 2 — skeleton + broken-image (CSS + minimal JS, always on)

- **CSS** (`static/css/app.css`): `.reading-pane-article img` gets the skeleton
  background while `[data-img-state="loading"]`; a dimensionless image gets a
  temporary `aspect-ratio: 16 / 9` so the skeleton block is visible (replaced by
  the real ratio once the image's natural size is known on load). A
  `.reading-pane-article img[data-img-state="broken"]` (or a sibling
  `.rp-broken-image` element) renders the dashed-box fallback.
- **JS** (`static/js/app.js`): after a reading-pane render, a small pass over
  `.reading-pane-article img` attaches `load`/`error` handlers:
  - `load` → set `data-img-state="loaded"` (clears skeleton); if the image had
    no dimensions, optionally drop the temporary `aspect-ratio` so it shows at
    natural size.
  - `error` → set `data-img-state="broken"` and render the fallback box (reuse
    the image's `alt`). Keep the reserved height.
  This runs in the existing swap/render lifecycle (where `cancelPaneImages` and
  pane wiring already live). No bundler — vanilla ES module only.

### Layer 3 — in-memory dimension cache + background measurement

The root fix for the 60%. **In-memory only**, mirroring `summary_cache`
(moka) — no DB table, no migration, no persistence. Re-measuring after a restart
is cheap and bounded (only viewed images are ever measured).

- **Cache** (`services/image_dimensions.rs` or fold into a new small module):
  `moka` `Cache<String /*original url*/, Option<(u32,u32)>>`. `Some` = measured
  dims; `None` = negative result (not an image / failed / undecodable) to avoid
  re-fetching. Bounded capacity, LRU. Lives in `AppState`.
- **Measurement worker** (`services/image_dimension_worker.rs`, modeled on
  `summary_worker.rs`): an `mpsc` queue of original image URLs; a `JoinSet`
  concurrency cap (≈4). For each URL: validate via `utils/url_validation`
  (SSRF), issue a **ranged GET** (`Range: bytes=0-65535`, fall back to full GET
  if the server ignores Range), parse the header with the `imagesize` crate
  (jpeg/png/gif/webp), write the result (or `None`) into the cache. Dedup
  in-flight URLs; time out slow fetches → negative result.
- **Render-time integration:** `sanitize`'s `img_handler` looks up the cache by
  original URL (step 3 above) and injects dims on hit. On miss it records the
  URL; the reading-pane builder (which has `AppState`) sends the collected
  misses to the worker queue (best-effort, non-blocking). Next render of any
  entry containing that image gets the cached dims → reserved space.
- **First-view caveat (accepted):** the first time an entry is opened its
  dimensionless images aren't measured yet, so that view may still shift once;
  subsequent views are stable. Layer 2's skeleton covers the visual in the
  meantime. (True first-view zero-CLS would require sync-time baking, which was
  considered and rejected for its sync/network coupling and backfill cost.)

### Sanitize interface change

`sanitize_html` becomes able to (a) read the dimension cache and (b) report
dimensionless-image URLs it couldn't resolve. To keep the function focused and
its other callers unaffected, pass an optional context, e.g.:

```rust
pub struct ImageDimensionCtx<'a> {
    cache: &'a ImageDimensionCache,
    misses: &'a mut Vec<String>, // original URLs with no known dims
}
sanitize_html(content, secret, base_url, referrer, proxy_base_url, Option<ImageDimensionCtx>)
```

Callers that don't reserve space (if any) pass `None`. The reading-pane builder
passes `Some`, then enqueues `misses`. Keep the harvest + Layer-1 behavior
active even when the ctx is `None` (they need no cache).

## Error Handling

- Image load failure (proxy/404/SSRF) → JS `error` → dashed-box fallback,
  reserved height retained.
- Measurement failure (non-image, timeout, bad header, SSRF-blocked) → cache
  `None` so it is not retried; the image keeps Layer 2's temporary ratio.
- Worker queue full / send error → best-effort drop (image just stays
  undimensioned; Layer 2 still applies). Never blocks the render.

## Testing

**Rust (`cargo nextest run`):**
- sanitize: `width`/`height` survive (Layer 1); hint harvest from
  `data-original-width/height` and inline `style`; `srcset` is stripped (no
  proxy bypass); dimension injection from a pre-seeded cache; miss collection.
- measurement: `imagesize` parses jpeg/png/gif/webp headers from a byte prefix;
  SSRF-blocked URL yields a negative result; negative results are cached;
  in-flight dedup.
**E2E (Playwright BDD):**
- A reading-pane image shows the skeleton state then `loaded` after load.
- A broken image URL renders the dashed-box fallback with its alt text.
- An image with `width`/`height` reserves space (no post-load size jump).
**Screenshots:** the default captures use already-loaded demo images, so they
should not change; verify and update only if they do.

## Phasing (single spec, two implementation phases)

- **Phase 1 — no new infrastructure:** Layer 1 (confirm + test), dimension-hint
  harvest, Layer 2 (CSS skeleton + JS load/error + broken fallback). Delivers
  the skeleton, broken-image handling, and removes the blank collapse for
  hint/dimensioned images immediately.
- **Phase 2 — dimension cache + worker:** the in-memory cache, the measurement
  worker, the `sanitize` ctx change, and the builder enqueue. Pushes the 60% to
  eventual zero-CLS.

## Implementation Notes / Risks

- `imagesize` crate: verify it is published ≥7 days before adding (dependency
  policy). Pure-Rust header parser; no native deps.
- The harvest must read the `data-*` hints **before** ammonia strips them — do
  it in the existing `promote_lazy_images`/pre-ammonia stage or extend that pass,
  not in `rewrite_post_ammonia` (where the hints are already gone).
- Distinguish content images from tiny inline icons in Layer 2: only apply the
  temporary `aspect-ratio`/skeleton to images without explicit dims AND not
  obviously tiny (e.g., skip when a harvested/known dimension is < ~32px, or
  scope to block-level images). Avoid blowing up emoji/inline glyphs.
- Keep the proxy passthrough and SSRF guard authoritative for the worker's
  fetches (reuse `utils/url_validation`).

## Files Touched (summary)

- `src/services/sanitize.rs` — hint harvest, optional dimension ctx, cache
  lookup + miss collection.
- `src/services/image_dimensions.rs` (new) — moka cache type.
- `src/services/image_dimension_worker.rs` (new) — measurement worker.
- `src/services/mod.rs` — exports.
- `src/lib.rs` — `AppState` fields (cache + worker tx); start worker wiring lives
  in `src/main.rs`.
- `src/handlers/entries.rs` (`build_reading_pane_view`) — pass dimension ctx to
  `sanitize_html`, enqueue misses.
- `static/css/app.css` — skeleton + broken-image styles.
- `static/js/app.js` — per-image `load`/`error` lifecycle.
- `templates/` — only if the broken-image fallback needs a markup hook.
- Tests under `tests/`, sanitize unit tests, and `e2e/`.
- `Cargo.toml` — `imagesize` (cooldown-checked).
