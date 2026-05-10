# SSR-first PR-9: /search Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate `/search` from CSR shell (`<rdrs-entries-page>` mode `search`, served by `static/js/pages/entries.js`) to direct SSR. The `?q=` query string drives a server-side search via the existing `entry::list_by_user(conn, user_id, &EntryFilter { search: Some(q), .. }, EntrySortOrder::PublishedAt, 50, 0)`. Results render as a simple list (title → external link, feed name, published date, optional snippet). No reading-pane integration in this PR — that lives in PR-10's swap helper.

`entries.js` stays alive (PR-10/11 still depend on it for `/`, `/entries`, `/entries/{read,starred,summarized}`, `/feeds/{id}/entries`, `/categories/{id}/entries`), but its `mode === 'search'` branch becomes unreachable. PR-12 cleanup removes that dead code along with the rest of `entries.js`.

**Architecture:** Single commit. T1 rewrites `templates/search.html`, extends `SearchTemplate`, and rewrites `search_page` handler. No new routes, no JSON endpoint deletions.

**Tech Stack:** Rust + Axum + Askama. `crate::models::entry::list_by_user` already supports `search: Option<String>` on `EntryFilter` (LIKE `%q%` on title + content, case-insensitive).

**Spec:** `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`

**Branch:** `feat/ssr-search-page` (already created off updated `main` at commit `887f3e1`).

---

## Pre-flight

- [x] Verify branch + clean state.
- [ ] Source OpenSSL env: `source /tmp/rdrs-env.sh`.
- [ ] Baseline: `cargo nextest run` → expect post-PR-8 baseline green.

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify | `templates/search.html` | Full SSR replacement — search input + results list. |
| Modify | `src/handlers/pages.rs` | Extend `SearchTemplate` (q + results); rewrite `search_page` handler. |
| Modify | `tests/pages_test.rs` | Update `test_search_page` to assert SSR content; add `test_search_page_with_results`. |

No `src/lib.rs` changes (`GET /search` already wired). No JS deletions in this PR — `entries.js` stays for PR-10/11. The `mode === 'search'` branch in `entries.js` becomes unreachable dead code; PR-12 deletes the whole entries module.

---

## Task 1: SSR /search

### Steps

- [ ] **Step 1: Rewrite `templates/search.html`.**

  Replace with full SSR: form at top, results below.

  ```html
  {% extends "app_layout.html" %}

  {% block page %}
      <div class="app-layout">
          <rdrs-sidebar active="search"></rdrs-sidebar>
          <main class="main-content">
              <div class="page-content">
                  <rdrs-flash class="flash-container"></rdrs-flash>
                  <h1>Search</h1>
                  <form method="get" action="/search" class="search-form">
                      <div class="form-group">
                          <input type="text" name="q" value="{{ q }}" placeholder="Search entries..." autofocus required data-testid="search-input">
                      </div>
                      <button type="submit" data-testid="search-btn">Search</button>
                  </form>
                  {% if q.is_empty() %}
                      <p class="muted">Enter a search term and press Enter to search.</p>
                  {% else if results.is_empty() %}
                      <p class="muted">No results for "{{ q }}".</p>
                  {% else %}
                      <ul class="search-results" data-testid="search-results">
                          {% for r in results %}
                              <li class="search-result">
                                  <a href="{{ r.link }}" target="_blank" rel="noopener noreferrer" class="search-result-title">{{ r.title }}</a>
                                  <div class="search-result-meta">
                                      <span class="muted">{{ r.feed_title }}</span>
                                      &middot;
                                      <span class="muted">{{ r.published_relative }}</span>
                                  </div>
                                  {% if !r.snippet.is_empty() %}
                                      <p class="search-result-snippet">{{ r.snippet }}</p>
                                  {% endif %}
                              </li>
                          {% endfor %}
                      </ul>
                  {% endif %}
              </div>
          </main>
      </div>
  {% endblock %}
  ```

  Notes:
  - Result title links to `r.link` (the external article URL). Reading-pane integration will come in PR-10's swap helper.
  - `target="_blank" rel="noopener noreferrer"` so users don't lose their search results when opening an article.
  - Result limit: 50 (no pagination — keeps PR-9 minimal).

- [ ] **Step 2: Extend `SearchTemplate` and rewrite `search_page` in `src/handlers/pages.rs`.**

  Add `SearchResultView` struct (id, link, title, feed_title, published_relative, snippet). Extend `SearchTemplate` with `q: String` + `results: Vec<SearchResultView>`. Drop the `<rdrs-entries-page>` shell pattern.

  Handler:

  ```rust
  #[derive(serde::Deserialize)]
  pub struct SearchQuery {
      pub q: Option<String>,
  }

  pub async fn search_page(
      auth_user: PageAuthUser,
      State(state): State<AppState>,
      flash: Flash,
      Query(query): Query<SearchQuery>,
  ) -> (Flash, SearchTemplate) {
      let layout = build_app_layout(&state, &auth_user, &flash).await;
      let q = query.q.unwrap_or_default().trim().to_string();
      let user_id = auth_user.user.id;

      let results = if q.is_empty() {
          Vec::new()
      } else {
          let q_for_filter = q.clone();
          state
              .db
              .read_user(move |conn| {
                  let filter = entry::EntryFilter {
                      search: Some(q_for_filter),
                      ..Default::default()
                  };
                  let entries = entry::list_by_user(
                      conn,
                      user_id,
                      &filter,
                      entry::EntrySortOrder::PublishedAt,
                      50,
                      0,
                  )?;
                  Ok::<_, AppError>(
                      entries
                          .into_iter()
                          .map(|e| SearchResultView {
                              id: e.id,
                              link: e.link.clone().unwrap_or_else(|| format!("/entries/{}", e.id)),
                              title: e.title.clone().unwrap_or_else(|| "(no title)".to_string()),
                              feed_title: e.feed_title.clone(),
                              published_relative: format_relative_time(e.published_at).0,
                              snippet: build_snippet(e.content.as_deref().or(e.summary.as_deref()), 200),
                          })
                          .collect::<Vec<_>>(),
                  )
              })
              .await
              .ok()
              .and_then(|r| r.ok())
              .unwrap_or_default()
      };

      (flash, SearchTemplate {
          title: "Search",
          git_version: crate::GIT_VERSION,
          layout,
          q,
          results,
      })
  }
  ```

  `build_snippet` — small helper that strips HTML and truncates to ~200 chars. Implement as plain text strip (regex `<[^>]+>` → ""), trim, then take first N chars + "…" if longer.

  **VERIFY:**
  - `EntryWithFeed` field names: confirm `feed_title`, `link`, `published_at`, `content`, `summary`, `id`. Read `src/models/entry.rs` lines 30-50 for the struct. Adjust if names differ.

- [ ] **Step 3: Update `tests/pages_test.rs::test_search_page` + add new tests.**

  Drop the CSR-shell assertions (`<rdrs-entries-page>`, `entries.js` script tag). Add:
  - `test_search_page_renders_form` — GET `/search` (no q) → form visible, "Enter a search term" message
  - `test_search_page_with_results` — seed feed + entries with matching titles → GET `/search?q=Foo` → titles appear in HTML
  - `test_search_page_no_results` — GET `/search?q=zzznotfoundzzz` → "No results for" message

- [ ] **Step 4: Compile + test.**

  `source /tmp/rdrs-env.sh && cargo nextest run`. Expect 700 baseline ± delta.

- [ ] **Step 5: Format + commit + push + PR.**

  `cargo fmt`. Squash-style commit message. PR title: `feat(ssr): SSR-first PR-9 — /search page`.

---

## Wrap-up

Subsequent PRs:
- PR-10: entries family — `/`, `/entries`, `/entries/{read,starred,summarized}` SSR with reading-pane swap helper (High risk — biggest single page move)
- PR-11: `/feeds/{id}/entries`, `/categories/{id}/entries`
- PR-12: cleanup (delete `entries.js` and friends)
