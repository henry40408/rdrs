# Unify empty-state style

Date: 2026-05-25
Status: Approved

## Problem

Empty states across the app are visually inconsistent. There are effectively
two styles at three scales:

- **Fancy editorial** (`.search-status`): centered italic Source Serif with an
  `⁂` asterism mark and generous whitespace — used only on `/search`.
- **Semi-fancy** (`.reading-pane-empty`): centered italic muted, no mark —
  reading pane only.
- **Minimal** (`.muted` plain text, sometimes inside a `<td>`): the entries
  list, `/feeds`, `/categories`, `/admin`, and `/statistics` sub-sections.

The goal is a single, modern, consistent empty-state language.

## Decision

Adopt the **editorial-modern ("D")** direction: a thin gold accent rule above a
serif heading with an italic muted subtext. **No icon, no card, no button** —
simple elements only. The ornate `⁂` asterism is dropped in favor of a clean
40×3px accent rule.

Two tiers of the same family, so the system reads as one voice across scales:

### Tier 1 — `.empty-state` (full-surface)

Used on the entries-family list pages and `/search`. Structure:

```
        ▬               accent rule  (40×3px, --color-accent, opacity .8)
   <heading>            serif, --font-2xl, weight 600
   <subtext>            italic, muted, --font-lg, line-height 1.6
```

Centered, `max-width: 30rem`, top whitespace via `margin`/`padding-top`.
Carries an `.empty-state-kbd` inline element (migrated from `.search-status-kbd`)
for keyboard hints.

### Tier 2 — `.empty-state-compact` (narrow surfaces)

Used in the reading pane, table cells (`/feeds`, `/categories`, `/admin`), and
`/statistics` sub-sections — places where a full editorial block would break the
layout. A single centered italic muted line (`--font-base`), no rule, no heading.
This is the natural collapse of Tier 1 into tight space.

## Copy

### Tier 1 (heading + subtext)

| Location | Heading | Subtext |
|----------|---------|---------|
| Unread cleared | All caught up | You've read every unread entry — new items land here as your feeds refresh. |
| All entries empty | Nothing to read yet | Subscribe to a few feeds and their entries will gather here. |
| Read empty | No read entries yet | Entries stay here once you've opened and read them. |
| Starred empty | No starred entries | Star an entry and it'll wait for you here. |
| Summarized empty | No summaries yet | Entries you summarize are collected on this page. |
| Category empty | Nothing in this category | The feeds in this category haven't brought in any entries yet. |
| Feed empty | Nothing in this feed | This feed hasn't published anything yet, or it's still syncing. |
| Search — initial | Search your library | Type a keyword and press `Enter` to find entries by title or content. |
| Search — no results | No matches | Nothing matched "{{ q }}". Try another keyword or check the spelling. |

### Tier 2 (single line)

| Location | Line |
|----------|------|
| Reading pane | Select an entry from the list to start reading. |
| Feeds table | No feeds yet — add one using the form above. |
| Categories table | No categories yet — create one using the form above. |
| Admin users | No users found. |
| Statistics (×2) | No entries in this period. |

## Implementation surface

- **`static/css/app.css`** — replace `.search-status` / `.search-status::before`
  / `.search-status-kbd` with the generic `.empty-state` / `.empty-state::before`
  / `.empty-state-kbd`; add `.empty-state-compact`. Fold `.reading-pane-empty`'s
  text styling onto the compact class (keep its flex vertical-centering wrapper).
- **`src/handlers/pages.rs`** — split `EntriesLayoutContext.empty_message:
  &'static str` into `empty_title: &'static str` + `empty_detail: &'static str`.
  Update all 8 construction sites with the copy above.
- **Templates**
  - `_entries_layout.html` — list empty state renders Tier 1
    (`empty_title` + `empty_detail`); the reading-pane placeholder renders Tier 2.
  - `search.html` — both states render Tier 1; `.search-status*` → `.empty-state*`.
  - `feeds.html`, `categories.html`, `admin.html` — `<td colspan>` empty rows
    render Tier 2.
  - `statistics.html` — the two `No entries in this period` placeholders render
    Tier 2.
- **Tests**
  - `tests/pages_test.rs` — update assertions tied to old copy / `.search-status`;
    assert new headings + `.empty-state` markup.
  - Playwright BDD — the `search-empty` testid stays; update any asserted text
    (e.g. "Nothing matched") to the new copy.

## Out of scope

- No icons, cards, or CTA buttons.
- No new build tooling (vanilla CSS only).
- `IMAGE_PROXY_SECRET` and other settings work is unrelated.

## Testing strategy

- Unit/integration (`cargo nextest run`): each list page renders its heading +
  detail; search renders both states; `.empty-state` present, old `.search-status`
  absent.
- e2e (Playwright BDD): search no-result path shows the new copy under the
  existing `search-empty` testid.
