# SSR-first PR-6: /statistics Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate `/statistics` from CSR shell + JSON API to direct SSR. The page is read-only — period selector is already `<a href="...">`, custom-date is already `<form method="get">` — so no form-action endpoints are needed. The handler reads query params, computes data via the existing `crate::models::statistics::*` helpers, and renders Askama markup directly. `static/js/pages/statistics.js` and `GET /api/statistics` (plus all its DTO structs) are deleted.

**Architecture:** Single commit (after the plan commit). The existing `get_statistics` JSON handler in `src/handlers/statistics.rs` does most of the heavy lifting via `crate::models::statistics`; the new SSR handler reuses those helpers and assembles a `StatisticsTemplate` that an Askama template renders. The custom-date form's existing JS submit-interceptor (which currently calls `window.rdrsNavigate`) is no longer needed — native form submit + full reload Just Works.

**Tech Stack:** Rust + Axum + Askama. No new deps. `chrono` for dates, already a dep.

**Spec:** `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`

**Branch:** `feat/ssr-statistics-page` (already created off updated `main` at commit `4230b1e`).

---

## Pre-flight

- [ ] **Verify branch and clean state.**

  Run: `git status && git branch --show-current && git log -3 --oneline`
  Expected: branch `feat/ssr-statistics-page`, working tree clean, latest commit on main is `4230b1e feat(ssr): SSR-first PR-5 — /admin page (#190)`.

- [ ] **Source OpenSSL env.**

  Run: `source /tmp/rdrs-env.sh`

- [ ] **Baseline test suite green.**

  Run: `cargo nextest run`
  Expected: 696/696 pass.

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify | `templates/statistics.html` | Full SSR replacement with the same DOM structure as today's CSR `_renderContent` produces. |
| Modify | `src/handlers/pages.rs` | Extend `StatisticsTemplate` with all the data fields the template needs; rewrite `statistics_page` handler to call `crate::models::statistics::*` helpers and populate the template. |
| Modify | `src/lib.rs` | Drop `.route("/api/statistics", get(handlers::statistics::get_statistics))`. |
| Modify | `src/handlers/statistics.rs` | DELETE `get_statistics` function and all DTO structs (`OverviewDto`, `DailyReadDto`, `CategoryCountDto`, `FeedCountDto`, `AdminStatsDto`, `StatisticsResponse`). Module becomes empty — DELETE the file and its `pub mod statistics;` line in `src/handlers/mod.rs`. |
| Modify | `src/handlers/mod.rs` | Remove `pub mod statistics;` line. |
| Delete | `static/js/pages/statistics.js` | Page module gone. |
| Modify | `src/handlers/static_assets.rs` | Drop `js/pages/statistics.js` allowlist entry. |
| Modify | `tests/statistics_test.rs` | DELETE the `/api/statistics` test section (around lines 180+). KEEP the `/statistics` page tests (those just hit the page URL — they'll continue to work, possibly need assertion updates if they reference CSR markers). |

---

## Task 1: SSR /statistics

### Pre-flight done above; below are the implementation steps.

- [ ] **Step 1: Rewrite `templates/statistics.html`.**

  Replace the file's contents:

  ```html
  {% extends "app_layout.html" %}

  {% block page %}
      <div class="app-layout">
          <rdrs-sidebar active="statistics"></rdrs-sidebar>
          <main class="main-content">
              <div class="page-content">
                  <rdrs-flash class="flash-container"></rdrs-flash>
                  <div class="stats-header">
                      <h1>Statistics</h1>
                      <form class="stats-period" method="get" action="/statistics">
                          <a href="/statistics?period=7d" class="stats-period-btn{% if active_period == "7d" %} active{% endif %}">7d</a>
                          <a href="/statistics?period=30d" class="stats-period-btn{% if active_period == "30d" %} active{% endif %}">30d</a>
                          <a href="/statistics?period=90d" class="stats-period-btn{% if active_period == "90d" %} active{% endif %}">90d</a>
                          <a href="/statistics?period=all" class="stats-period-btn{% if active_period == "all" %} active{% endif %}">All</a>
                          <span class="stats-period-divider">|</span>
                          <input type="hidden" name="period" value="custom">
                          <input type="date" name="from" value="{{ custom_from }}" class="stats-date-input">
                          <span class="stats-period-dash">&mdash;</span>
                          <input type="date" name="to" value="{{ custom_to }}" class="stats-date-input">
                          <button type="submit" class="stats-period-btn">Apply</button>
                      </form>
                  </div>

                  <div class="stats-cards">
                      <div class="stats-card">
                          <div class="stats-card-value">{{ total_entries }}</div>
                          <div class="stats-card-label">Total Entries</div>
                      </div>
                      <div class="stats-card">
                          <div class="stats-card-value stats-card-success">{{ read_entries }}</div>
                          <div class="stats-card-label">Read</div>
                      </div>
                      <div class="stats-card">
                          <div class="stats-card-value stats-card-warning">{{ unread_entries }}</div>
                          <div class="stats-card-label">Unread</div>
                      </div>
                      <div class="stats-card">
                          <div class="stats-card-value">{{ "{:.1}"|format(read_rate) }}%</div>
                          <div class="stats-card-label">Read Rate</div>
                      </div>
                      <div class="stats-card">
                          <div class="stats-card-value">{{ starred_entries }}</div>
                          <div class="stats-card-label">Starred</div>
                      </div>
                      <div class="stats-card">
                          <div class="stats-card-value">{{ summaries }}</div>
                          <div class="stats-card-label">Summaries</div>
                      </div>
                  </div>

                  <div class="stats-section">
                      <h2>Daily Read Articles</h2>
                      {% if daily_max == 0 %}
                          <p class="muted">No read activity in this period</p>
                      {% else %}
                          <div class="stats-chart">
                              {% for d in daily_read_counts %}
                                  <div class="stats-bar-col" title="{{ d.date }}: {{ d.count }}">
                                      <div class="stats-bar" style="height: {{ d.height_percent }}%"></div>
                                      <div class="stats-bar-label">{{ d.short_label }}</div>
                                  </div>
                              {% endfor %}
                          </div>
                      {% endif %}
                  </div>

                  <div class="stats-columns">
                      <div class="stats-section">
                          <h2>Entries by Category</h2>
                          {% if categories.is_empty() %}
                              <p class="muted">No entries in this period</p>
                          {% else %}
                              {% for c in categories %}
                                  <div class="stats-bar-row">
                                      <div class="stats-bar-row-header">
                                          <span>{{ c.name }}</span>
                                          <span class="muted">{{ c.count }}</span>
                                      </div>
                                      <div class="stats-progress">
                                          <div class="stats-progress-fill" style="width: {{ c.width_percent }}%"></div>
                                      </div>
                                  </div>
                              {% endfor %}
                          {% endif %}
                      </div>
                      <div class="stats-section">
                          <h2>Top Feeds</h2>
                          {% if top_feeds.is_empty() %}
                              <p class="muted">No entries in this period</p>
                          {% else %}
                              {% for f in top_feeds %}
                                  <div class="stats-bar-row">
                                      <div class="stats-bar-row-header">
                                          <span>{{ f.title }}</span>
                                          <span class="muted">{{ f.count }}</span>
                                      </div>
                                      <div class="stats-progress">
                                          <div class="stats-progress-fill" style="width: {{ f.width_percent }}%"></div>
                                      </div>
                                  </div>
                              {% endfor %}
                          {% endif %}
                      </div>
                  </div>

                  {% if let Some(a) = admin %}
                  <div class="stats-admin-section">
                      <h2>Site-wide Statistics</h2>
                      <div class="stats-cards">
                          <div class="stats-card stats-card-admin">
                              <div class="stats-card-value">{{ a.total_users }}</div>
                              <div class="stats-card-label">Total Users</div>
                          </div>
                          <div class="stats-card stats-card-admin">
                              <div class="stats-card-value">{{ a.total_entries }}</div>
                              <div class="stats-card-label">Site Entries</div>
                          </div>
                          <div class="stats-card stats-card-admin">
                              <div class="stats-card-value">{{ a.total_feeds }}</div>
                              <div class="stats-card-label">Total Feeds</div>
                          </div>
                          <div class="stats-card stats-card-admin">
                              <div class="stats-card-value">{{ "{:.1}"|format(a.read_rate) }}%</div>
                              <div class="stats-card-label">Site Read Rate</div>
                          </div>
                      </div>
                  </div>
                  {% endif %}
              </div>
          </main>
      </div>
  {% endblock %}
  ```

  Notes:
  - Pre-computed values: `daily_max`, each `daily_read_counts[i].height_percent`, `daily_read_counts[i].short_label`, `categories[i].width_percent`, `top_feeds[i].width_percent`. The handler computes these so the template stays simple.
  - `{{ "{:.1}"|format(read_rate) }}` — Askama 0.15 supports the `|format` filter for printf-style formatting.
  - No `{% block page_script %}` — page is pure SSR.

- [ ] **Step 2: Rewrite `StatisticsTemplate` and `statistics_page` handler in `src/handlers/pages.rs`.**

  Locate the existing `StatisticsTemplate` (added in PR-2 Task 2 with `title`, `git_version`, `layout`). Replace with view-shape structs + the expanded template:

  ```rust
  pub struct DailyReadView {
      pub date: String,
      pub count: i64,
      pub height_percent: f64,
      pub short_label: String,
  }

  pub struct CategoryStatsView {
      pub name: String,
      pub count: i64,
      pub width_percent: f64,
  }

  pub struct FeedStatsView {
      pub title: String,
      pub count: i64,
      pub width_percent: f64,
  }

  pub struct AdminStatsView {
      pub total_users: i64,
      pub total_feeds: i64,
      pub total_entries: i64,
      pub read_rate: f64,
  }

  #[derive(Template)]
  #[template(path = "statistics.html")]
  pub struct StatisticsTemplate {
      pub title: &'static str,
      pub git_version: &'static str,
      pub layout: AppLayoutContext,
      pub active_period: String,
      pub custom_from: String,
      pub custom_to: String,
      pub total_entries: i64,
      pub read_entries: i64,
      pub unread_entries: i64,
      pub starred_entries: i64,
      pub summaries: i64,
      pub read_rate: f64,
      pub daily_max: i64,
      pub daily_read_counts: Vec<DailyReadView>,
      pub categories: Vec<CategoryStatsView>,
      pub top_feeds: Vec<FeedStatsView>,
      pub admin: Option<AdminStatsView>,
  }

  impl IntoResponse for StatisticsTemplate {
      fn into_response(self) -> Response {
          match self.render() {
              Ok(html) => Html(html).into_response(),
              Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
          }
      }
  }
  ```

  Rewrite the `statistics_page` handler. It should mirror the existing JSON `get_statistics` handler in `src/handlers/statistics.rs` — same DB calls, same admin-stats gating logic, same `chart_from` (90 days for `all`, otherwise `from`). The only difference is the output shape and the pre-computed `*_percent` / `short_label` / `daily_max` values for the template.

  ```rust
  pub async fn statistics_page(
      auth_user: PageAuthUser,
      State(state): State<AppState>,
      flash: Flash,
      Query(query): Query<StatisticsQuery>,
  ) -> (Flash, StatisticsTemplate) {
      let layout = build_app_layout(&state, &auth_user, &flash).await;

      let is_masquerading = auth_user.session.is_masquerading();
      let is_admin = if is_masquerading {
          auth_user.session.original_user_id.is_some()
      } else {
          auth_user.user.is_admin()
      };
      let show_admin_stats = is_admin && !is_masquerading;

      let (from, to, active_period) = resolve_statistics_period(&query);
      let chart_from = if active_period == "all" {
          let today = chrono::Utc::now().date_naive();
          (today - chrono::Duration::days(90)).to_string()
      } else {
          from.clone()
      };

      let user_id = auth_user.user.id;
      let from_c = from.clone();
      let to_c = to.clone();
      let chart_from_c = chart_from.clone();

      let (overview, daily, cats, feeds, admin_counts, admin_entry_stats) = state
          .db
          .read_user(move |c| {
              let overview = crate::models::statistics::get_personal_overview(
                  c, user_id, &from_c, &to_c,
              )
              .unwrap_or_default();
              let daily = crate::models::statistics::get_daily_read_counts(
                  c,
                  user_id,
                  &chart_from_c,
                  &to_c,
              )
              .unwrap_or_default();
              let cats = crate::models::statistics::get_entries_by_category(
                  c, user_id, &from_c, &to_c,
              )
              .unwrap_or_default();
              let feeds = crate::models::statistics::get_top_feeds(
                  c, user_id, &from_c, &to_c, 10,
              )
              .unwrap_or_default();
              let admin_counts = if show_admin_stats {
                  crate::models::statistics::get_admin_counts(c).ok()
              } else {
                  None
              };
              let admin_entry_stats = if show_admin_stats {
                  crate::models::statistics::get_admin_entry_stats(c, &from_c, &to_c).ok()
              } else {
                  None
              };
              Ok::<_, AppError>((
                  overview,
                  daily,
                  cats,
                  feeds,
                  admin_counts,
                  admin_entry_stats,
              ))
          })
          .await
          .ok()
          .and_then(|r| r.ok())
          .unwrap_or_default();

      let (custom_from, custom_to) = if active_period == "custom" {
          (
              query.from.clone().unwrap_or_default(),
              query.to.clone().unwrap_or_default(),
          )
      } else {
          (String::new(), String::new())
      };

      let daily_max = daily.iter().map(|d| d.count).max().unwrap_or(0);
      let cat_max = cats.iter().map(|c| c.count).max().unwrap_or(0);
      let feed_max = feeds.iter().map(|f| f.count).max().unwrap_or(0);

      let daily_read_counts = daily
          .into_iter()
          .map(|d| {
              let date_str = d.date.format("%Y-%m-%d").to_string();
              let short_label = if date_str.len() >= 10 {
                  format!("{}/{}", &date_str[5..7], &date_str[8..10])
              } else {
                  date_str.clone()
              };
              let height_percent = if daily_max > 0 {
                  (d.count as f64 * 100.0) / daily_max as f64
              } else {
                  0.0
              };
              DailyReadView {
                  date: date_str,
                  count: d.count,
                  height_percent,
                  short_label,
              }
          })
          .collect();

      let categories = cats
          .into_iter()
          .map(|c| CategoryStatsView {
              count: c.count,
              width_percent: if cat_max > 0 {
                  (c.count as f64 * 100.0) / cat_max as f64
              } else {
                  0.0
              },
              name: c.name,
          })
          .collect();

      let top_feeds = feeds
          .into_iter()
          .map(|f| FeedStatsView {
              count: f.count,
              width_percent: if feed_max > 0 {
                  (f.count as f64 * 100.0) / feed_max as f64
              } else {
                  0.0
              },
              title: f.title,
          })
          .collect();

      let admin = match (admin_counts, admin_entry_stats) {
          (Some(c), Some(e)) => Some(AdminStatsView {
              total_users: c.total_users,
              total_feeds: c.total_feeds,
              total_entries: e.total_entries,
              read_rate: e.read_rate(),
          }),
          _ => None,
      };

      (
          flash,
          StatisticsTemplate {
              title: "Statistics",
              git_version: crate::GIT_VERSION,
              layout,
              active_period,
              custom_from,
              custom_to,
              total_entries: overview.total_entries,
              read_entries: overview.read_entries,
              unread_entries: overview.unread_entries(),
              starred_entries: overview.starred_entries,
              summaries: overview.summaries,
              read_rate: overview.read_rate(),
              daily_max,
              daily_read_counts,
              categories,
              top_feeds,
              admin,
          },
      )
  }
  ```

  **VERIFY:** the helper names against `src/handlers/statistics.rs:65-178` — they should be the same set:
  - `crate::models::statistics::get_personal_overview`
  - `crate::models::statistics::get_daily_read_counts`
  - `crate::models::statistics::get_entries_by_category`
  - `crate::models::statistics::get_top_feeds`
  - `crate::models::statistics::get_admin_counts`
  - `crate::models::statistics::get_admin_entry_stats`

  Also `overview.unread_entries()`, `overview.read_rate()`, `e.read_rate()` are method calls — verify they exist on the personal overview struct + admin entry stats struct.

  Add any needed `use` lines at the top of `src/handlers/pages.rs` (e.g. `use axum::extract::Query;` if not already present).

- [ ] **Step 3: Drop `/api/statistics` route from `src/lib.rs`.**

  Remove:
  ```rust
          .route("/api/statistics", get(handlers::statistics::get_statistics))
  ```

- [ ] **Step 4: Delete `src/handlers/statistics.rs` and remove from `src/handlers/mod.rs`.**

  ```bash
  git rm src/handlers/statistics.rs
  ```

  Edit `src/handlers/mod.rs` — remove the `pub mod statistics;` line.

  Verify no leftover references:
  ```bash
  grep -rn "handlers::statistics" src/
  ```
  Expected: zero matches.

- [ ] **Step 5: Drop `js/pages/statistics.js` allowlist entry from `src/handlers/static_assets.rs`.**

  Remove:
  ```rust
      (
          "js/pages/statistics.js",
          include_str!("../../static/js/pages/statistics.js"),
      ),
  ```

- [ ] **Step 6: Delete `static/js/pages/statistics.js`.**

  ```bash
  git rm static/js/pages/statistics.js
  ```

- [ ] **Step 7: Update `tests/statistics_test.rs`.**

  Find the `/api/statistics` test section (around lines 180+). DELETE every test that exercises `app.server.get("/api/statistics...")`. The page-level tests (those that hit `/statistics` and assert on rendered HTML) STAY but may need assertions updated:
  - Replace any assertion on `<rdrs-statistics-page>` or `/static/js/pages/statistics.js` with assertions on the SSR-rendered table content (e.g. `<h1>Statistics</h1>`, `Total Entries`, `Daily Read Articles`).
  - Tests that previously asserted on the JSON shape via `/api/statistics` are GONE.

  After this step, all remaining tests in `statistics_test.rs` should be page-level (hitting `/statistics`) or unrelated.

  Look for `test_statistics_page_shell_renders_for_user` and `test_statistics_shell_embeds_sidebar_bootstrap` (added in PR-2 reviews around line 700+ of pages_test.rs — actually those might be elsewhere). Update / rename if they reference the CSR shell.

- [ ] **Step 8: Compile + test.**

  Run: `cargo nextest run`
  Expected: full suite green. Test count delta will be a net decrease (deleted /api/statistics tests + 1-2 new SSR-content asserts).

  Common failures:
  - Askama `|format` filter syntax mismatch — try `{{ "{:.1}"|format(read_rate) }}` first; if it fails, use `{{ read_rate }}` and accept the float's default Display formatting (acceptable visual change).
  - `{% if let Some(a) = admin %}` syntax — already used elsewhere in templates, should work.

- [ ] **Step 9: Verify cleanup.**

  ```bash
  grep -rn "rdrs-statistics-page\|/api/statistics\|/static/js/pages/statistics.js\|handlers::statistics" src/ templates/ static/ tests/
  ```
  Acceptable: zero in `src/`, `templates/`, `static/`. In `tests/`, only the new test's negative assertions (if any).

- [ ] **Step 10: Format and commit.**

  Run: `cargo fmt`
  ```bash
  git add templates/statistics.html src/handlers/pages.rs src/handlers/mod.rs src/handlers/static_assets.rs src/lib.rs tests/statistics_test.rs
  # `git rm src/handlers/statistics.rs` and `git rm static/js/pages/statistics.js` are already staged.
  git commit -m "$(cat <<'EOF'
  feat(ssr): SSR /statistics — drop CSR element + JSON endpoint

  /statistics now renders directly from the DB. Period buttons are
  plain <a href="?period=...">; the custom-date form is a native
  GET form. Pre-computes height_percent / width_percent / daily_max
  so the template stays simple.

  Deletes static/js/pages/statistics.js, src/handlers/statistics.rs
  (entire module — was a single GET /api/statistics endpoint),
  the <rdrs-statistics-page> element, and all the API DTO structs.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Wrap-up

- [ ] **Final sweep.**

  Run: `cargo nextest run && cargo fmt --check`
  Expected: tests green, formatting clean.

- [ ] **Push branch.**

  Run: `git push -u origin feat/ssr-statistics-page`

- [ ] **Open the PR.**

  ```bash
  gh pr create --title "feat(ssr): SSR-first PR-6 — /statistics page" --body "$(cat <<'EOF'
  ## Summary

  Migrates `/statistics` to SSR. The page is read-only — period buttons are already `<a href>` and the custom-date form is already `<form method="get">` — so no form-action endpoints are needed. Handler reads query params, computes data via the existing `crate::models::statistics::*` helpers, and pre-computes `height_percent` / `width_percent` / `daily_max` so the Askama template stays simple.

  Drops `static/js/pages/statistics.js` (200 lines), the entire `src/handlers/statistics.rs` module (the single `GET /api/statistics` endpoint plus its DTO structs), and the `<rdrs-statistics-page>` element.

  ## Test plan

  - [x] `cargo nextest run` — full suite green.
  - [x] Page-level tests in `tests/statistics_test.rs` (and `tests/pages_test.rs`) pass; CSR-marker assertions replaced with SSR-content assertions.
  - [x] `/api/statistics` API tests deleted (endpoint gone).

  Spec: `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`
  Plan: `docs/superpowers/plans/2026-05-09-ssr-first-pr6-statistics-page.md`

  🤖 Generated with [Claude Code](https://claude.com/claude-code)
  EOF
  )"
  ```

---

## Subsequent PRs

Per the spec migration table, PR-7 is `/categories` SSR.
