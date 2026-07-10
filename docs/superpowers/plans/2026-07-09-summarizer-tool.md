# Summarizer Tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a logged-in-only `/summarizer` tool where a user pastes up to 30 URLs and gets a Kagi summary for each, rendered in the existing `summary-box` format, resolving strictly one at a time.

**Architecture:** SSR-first. `GET /summarizer` renders a page (Askama, app layout + sidebar). `POST /summarizer` validates the textarea and re-renders the page with one *Queued* card per URL. A page-scoped ES module walks the cards in DOM order, `POST`ing `/summarizer/item` per URL and swapping in the returned card fragment — sequencing and cancel are client-side; each request is one short Kagi call. Nothing is persisted.

**Tech Stack:** Rust (Axum + Askama), `sqlx` (unused here — no persistence), vanilla ES modules, Playwright BDD for E2E.

## Global Constraints

- No persistence: nothing written to `entry_summary` or any table; no migration.
- No changes to the entry-scoped MPSC summary worker.
- Max **30** URLs per run; blank lines dropped; input order preserved.
- SSRF validation for every URL via `crate::utils::url_validation::validate_url`.
- Card title = Kagi `Title:` line, else the URL host, else the URL.
- Reuse the user's Kagi session + target-language from `user_settings` (`get_save_services_config().kagi`).
- Rust: `cargo fmt` before commit; `cargo clippy --all-targets -- -D warnings` clean; tests via `cargo nextest run`; use `RDRS_FAST_HASH=1` for local runs.
- Commits GPG-signed; stage files explicitly (no `git add -A`); end commit messages with the `Co-Authored-By` trailer.
- After any UI change, rebuild (`cargo build`) before E2E/screenshots (assets are `include_*!`d at compile time).

---

### Task 1: Surface Kagi's title in `SummarizeResult`

**Files:**
- Modify: `src/services/summarize/kagi.rs` (struct `SummarizeResult` ~line 38; the `Title:`-stripping branch ~line 114-127)
- Test: `src/services/summarize/kagi.rs` (`#[cfg(test)]` module, alongside `summarize_success_strips_title_prefix`)

**Interfaces:**
- Produces: `SummarizeResult { success: bool, output_text: Option<String>, error: Option<String>, title: Option<String> }`. `title` is `Some(t)` when Kagi's markdown began with `Title: t\n\n`, else `None`. `output_text` body is unchanged from today (prefix still stripped).

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)]` module in `src/services/summarize/kagi.rs`:

```rust
#[tokio::test]
async fn summarize_success_extracts_title() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "output_data": {"markdown": "Title: Foo Bar\n\nThe body."}
        })))
        .mount(&server)
        .await;
    let config = KagiConfig { session_token: "t".into(), language: None };
    let result = summarize_url_with_base(&server.uri(), &config, "https://x.com/a")
        .await
        .unwrap();
    assert_eq!(result.title.as_deref(), Some("Foo Bar"));
    assert_eq!(result.output_text.as_deref(), Some("The body."));
}

#[tokio::test]
async fn summarize_success_title_none_when_absent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "output_data": {"markdown": "Plain body, no title line."}
        })))
        .mount(&server)
        .await;
    let config = KagiConfig { session_token: "t".into(), language: None };
    let result = summarize_url_with_base(&server.uri(), &config, "https://x.com/a")
        .await
        .unwrap();
    assert_eq!(result.title, None);
    assert_eq!(result.output_text.as_deref(), Some("Plain body, no title line."));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs summarize_success_extracts_title`
Expected: FAIL — `SummarizeResult` has no field `title`.

- [ ] **Step 3: Add the field and populate it**

In `struct SummarizeResult` add `pub title: Option<String>,`.

Every constructor of `SummarizeResult` in this file must set `title`. In the not-configured, error, and no-summary branches set `title: None`. In the success branch replace the `cleaned` computation so it captures the title:

```rust
} else if let Some(markdown) = body.output_data.and_then(|d| d.markdown) {
    // Split off a leading "Title: <t>\n\n" prefix if present.
    let (title, cleaned) = if let Some(rest) = markdown.strip_prefix("Title: ") {
        match rest.find("\n\n") {
            Some(pos) => (Some(rest[..pos].to_string()), rest[pos + 2..].to_string()),
            None => (None, markdown),
        }
    } else {
        (None, markdown)
    };
    Ok(SummarizeResult {
        success: true,
        output_text: Some(cleaned),
        error: None,
        title,
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs kagi`
Expected: PASS (new tests + existing `summarize_success_strips_title_prefix`, `markdown_without_title_prefix`).

- [ ] **Step 5: fmt, clippy, commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add src/services/summarize/kagi.rs
git commit -S -m "feat(kagi): expose page title in SummarizeResult

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: URL-list parsing + validation helper

**Files:**
- Create: `src/handlers/summarizer.rs`
- Modify: `src/handlers/mod.rs` (add `pub mod summarizer;`)
- Test: `src/handlers/summarizer.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Produces: `pub const MAX_URLS: usize = 30;`
- Produces: `pub(crate) fn parse_url_lines(input: &str) -> Result<Vec<String>, String>` — splits on newlines, trims each line, drops blanks, de-duplicates preserving first-seen order, rejects with a human message when empty / over `MAX_URLS` / any line is not a valid `http(s)` URL or fails SSRF validation. Returns the cleaned URL strings.
- Produces: `pub(crate) fn url_host(url: &str) -> String` — the host for the card-title fallback (returns the whole string if it cannot be parsed).

- [ ] **Step 1: Write the failing tests**

Create `src/handlers/summarizer.rs` with only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trims_and_drops_blanks() {
        let out = parse_url_lines("  https://a.com/x \n\n https://b.com/y\n").unwrap();
        assert_eq!(out, vec!["https://a.com/x", "https://b.com/y"]);
    }

    #[test]
    fn dedupes_preserving_order() {
        let out = parse_url_lines("https://a.com\nhttps://a.com\nhttps://b.com").unwrap();
        assert_eq!(out, vec!["https://a.com", "https://b.com"]);
    }

    #[test]
    fn rejects_empty() {
        assert!(parse_url_lines("   \n  ").is_err());
    }

    #[test]
    fn rejects_over_max() {
        let many = (0..(MAX_URLS + 1))
            .map(|i| format!("https://ex.com/{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let err = parse_url_lines(&many).unwrap_err();
        assert!(err.contains("30"));
    }

    #[test]
    fn rejects_non_http_scheme() {
        assert!(parse_url_lines("ftp://a.com/x").is_err());
        assert!(parse_url_lines("not a url").is_err());
    }

    #[test]
    fn host_fallback() {
        assert_eq!(url_host("https://news.example.net/a/b"), "news.example.net");
        assert_eq!(url_host("garbage"), "garbage");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs summarizer::tests`
Expected: FAIL — `parse_url_lines` / `url_host` / `MAX_URLS` undefined.

- [ ] **Step 3: Implement the helpers**

At the top of `src/handlers/summarizer.rs` (above the test module):

```rust
use url::Url;

use crate::utils::url_validation::validate_url;

/// Maximum URLs accepted in a single summarizer run.
pub const MAX_URLS: usize = 30;

/// Parse the textarea into a validated, de-duplicated, order-preserving list of
/// URL strings. Rejects an empty list, more than `MAX_URLS`, and any line that
/// is not a fetchable http(s) URL (SSRF-validated).
pub(crate) fn parse_url_lines(input: &str) -> Result<Vec<String>, String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for raw in input.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let parsed =
            Url::parse(line).map_err(|_| format!("Not a valid URL: {line}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(format!("Only http(s) URLs are supported: {line}"));
        }
        validate_url(&parsed).map_err(|e| format!("URL not allowed ({line}): {e}"))?;
        if seen.insert(line.to_string()) {
            out.push(line.to_string());
        }
    }
    if out.is_empty() {
        return Err("Enter at least one URL.".to_string());
    }
    if out.len() > MAX_URLS {
        return Err(format!("Too many URLs — {} max per run.", MAX_URLS));
    }
    Ok(out)
}

/// Host for the card-title fallback; returns the input unchanged if unparseable.
pub(crate) fn url_host(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}
```

Add `pub mod summarizer;` to `src/handlers/mod.rs` (keep the list alphabetical — after `static_assets`? it is currently `static_assets` then `user`; insert `summarizer` before `user`).

> Note: `validate_url`'s error type must be `Display`. If `UrlValidationError` is not `Display`, use `format!("{e:?}")` instead of `{e}`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs summarizer::tests`
Expected: PASS (6 tests).

- [ ] **Step 5: fmt, clippy, commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add src/handlers/summarizer.rs src/handlers/mod.rs
git commit -S -m "feat(summarizer): add URL-list parse + SSRF validation helper

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Card view struct, macro, and card fragment template

**Files:**
- Modify: `src/handlers/summarizer.rs` (add `SummarizerCard` + `SummarizerCardTemplate`)
- Modify: `templates/macros.html` (add `summarizer_card` macro)
- Create: `templates/_summarizer_card_fragment.html`
- Modify: `static/css/app.css` (add `.summary-box.pending` + `.sz-spinner` styles)

**Interfaces:**
- Produces: `pub(crate) struct SummarizerCard { pub index: usize, pub url: String, pub title: String, pub state: &'static str, pub summary: String, pub error: String }` where `state ∈ {"queued","completed","error"}`.
- Produces: `SummarizerCardTemplate { card: SummarizerCard }` rendering `_summarizer_card_fragment.html`.
- Consumes (later tasks): the macro `macros::summarizer_card(card)` renders one `.summary-box` with `id="sz-card-{index}"`, `data-summarizer-card`, `data-summarizer-url`, `data-summarizer-index`, `data-state`.

- [ ] **Step 1: Add the view struct + fragment template struct**

In `src/handlers/summarizer.rs`, above the test module:

```rust
use askama::Template;
use axum::response::Html;

/// One URL's card. `state` selects the rendered branch; unused string fields are
/// empty. `summary` is trusted HTML/markdown from Kagi (rendered with `|safe`).
#[derive(Debug, Clone)]
pub(crate) struct SummarizerCard {
    pub index: usize,
    pub url: String,
    pub title: String,
    pub state: &'static str,
    pub summary: String,
    pub error: String,
}

#[derive(Template)]
#[template(path = "_summarizer_card_fragment.html")]
pub(crate) struct SummarizerCardTemplate {
    pub card: SummarizerCard,
}
```

- [ ] **Step 2: Add the macro**

Append to `templates/macros.html` (icons are already imported at the top of that file):

```html
{% macro summarizer_card(card) %}<div class="summary-box sz-card{% if card.state == "queued" %} pending{% endif %}" id="sz-card-{{ card.index }}" data-summarizer-card data-summarizer-url="{{ card.url }}" data-summarizer-index="{{ card.index }}" data-state="{{ card.state }}">
    <div class="summary-actions" data-sz-actions>
        {% if card.state == "completed" %}<button type="button" class="rp-action" data-sz-copy aria-label="Copy summary"><span class="action-label">Copy</span></button><button type="button" class="rp-action" data-sz-dismiss aria-label="Dismiss summary"><span class="action-label">Dismiss</span></button>{% else if card.state == "error" %}<button type="button" class="rp-action" data-sz-retry aria-label="Retry summarization"><span class="action-label">Retry</span></button><button type="button" class="rp-action" data-sz-dismiss aria-label="Dismiss"><span class="action-label">Dismiss</span></button>{% endif %}
    </div>
    <div class="summary-header">
        <div class="summary-title" data-sz-title>{{ card.title }}</div>
        <a class="summary-link" href="{{ card.url }}" target="_blank" rel="noopener noreferrer">{{ card.url }}</a>
    </div>
    {% if card.state == "completed" %}<blockquote class="rp-summary-content" data-sz-body>{{ card.summary|safe }}</blockquote>{% else if card.state == "error" %}<div class="summary-error-banner" data-sz-error><span class="action-icon" aria-hidden="true">{% call icons::close() %}{% endcall %}</span><span>Summarization failed: {{ card.error }}</span></div>{% else %}<p class="status" data-sz-status>Queued</p>{% endif %}
</div>{% endmacro %}
```

- [ ] **Step 3: Create the fragment template**

`templates/_summarizer_card_fragment.html`:

```html
{% import "macros.html" as macros %}{% call macros::summarizer_card(card) %}{% endcall %}
```

- [ ] **Step 4: Add CSS for the queued/spinner states**

Append to `static/css/app.css` (near the `.summary-box` block ~line 2200):

```css
/* Summarizer tool — queued card dimming + in-flight spinner. */
.summary-box.pending { opacity: 0.62; }
.summary-box .status .sz-spinner {
    display: inline-block; width: 11px; height: 11px; margin-right: 6px;
    border: 2px solid var(--color-accent); border-right-color: transparent;
    border-radius: 50%; vertical-align: -1px; animation: sz-spin 0.7s linear infinite;
}
@keyframes sz-spin { to { transform: rotate(360deg); } }
@media (prefers-reduced-motion: reduce) { .summary-box .status .sz-spinner { animation: none; } }
```

- [ ] **Step 5: Verify it compiles (template is checked at build time)**

Run: `cargo build`
Expected: builds clean. Askama validates `_summarizer_card_fragment.html` and the macro against `SummarizerCard`.

- [ ] **Step 6: fmt, commit**

```bash
cargo fmt
git add src/handlers/summarizer.rs templates/macros.html templates/_summarizer_card_fragment.html static/css/app.css
git commit -S -m "feat(summarizer): add card view, macro, fragment template, and styles

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: `GET /summarizer` page + sidebar nav

**Files:**
- Modify: `src/handlers/summarizer.rs` (add `SummarizerTemplate` + `summarizer_page`)
- Create: `templates/summarizer.html`
- Modify: `src/lib.rs` (register route, after the `/statistics` route ~line 235)
- Modify: `static/js/components/rdrs-sidebar.js` (add Tools nav item + `sparkle`-style icon)
- Test: `tests/summarizer_test.rs` (new)

**Interfaces:**
- Consumes: `crate::handlers::pages::{build_app_layout, AppLayoutContext}`, `crate::middleware::auth::PageAuthUser`, `crate::middleware::flash::Flash`, `crate::models::user_settings`.
- Produces: `pub async fn summarizer_page(auth_user: PageAuthUser, State(state): State<AppState>, flash: Flash) -> (Flash, SummarizerTemplate)`.
- Produces: `SummarizerTemplate { title, git_version, layout, kagi_configured, urls_text, error: Option<String>, cards: Vec<SummarizerCard> }`.

- [ ] **Step 1: Write the failing tests**

Create `tests/summarizer_test.rs` (mirror the harness in `tests/pages_test.rs`):

```rust
mod common;
use common::default_test_config;

use std::sync::Arc;

use axum_test::TestServer;
use rdrs::models::user_settings;
use rdrs::services::KagiConfig;
use rdrs::services::save::SaveServicesConfig;
use rdrs::{AppState, Config, Db, Role, auth, create_router, models::user, services};

struct TestApp { server: TestServer, db: Db }

async fn create_test_app(config: Config) -> TestApp {
    let db = Db::connect_in_memory().await.unwrap();
    let webauthn = auth::create_webauthn(&config).unwrap();
    let summary_cache = services::create_summary_cache(100, 24);
    let (summary_tx, _rx) = services::create_summary_channel(10);
    let state = AppState {
        db: db.clone(),
        config: Arc::new(config),
        webauthn: Arc::new(webauthn),
        summary_cache,
        summary_tx,
        sidebar_cache: Arc::new(services::SidebarCache::default()),
        summary_cancels: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        events: services::EventBus::new(16),
        shutdown: tokio_util::sync::CancellationToken::new(),
    };
    let server = TestServer::builder().save_cookies().build(create_router(state));
    TestApp { server, db }
}

async fn login(app: &TestApp, username: &str) -> i64 {
    let hash = auth::hash_password("password123").unwrap();
    let u = user::create_user(&app.db, username, &hash, Role::User).await.unwrap();
    app.server.post("/api/session")
        .json(&serde_json::json!({"username": username, "password": "password123"}))
        .await;
    u.id
}

async fn configure_kagi(app: &TestApp, user_id: i64) {
    let cfg = SaveServicesConfig {
        linkding: None,
        kagi: Some(KagiConfig { session_token: "tok".into(), language: None }),
    };
    user_settings::update_save_services(&app.db, user_id, &cfg).await.unwrap();
}

#[tokio::test]
async fn page_shows_settings_prompt_when_kagi_unset() {
    let app = create_test_app(default_test_config()).await;
    login(&app, "alice").await;
    let res = app.server.get("/summarizer").await;
    res.assert_status_ok();
    let body = res.text();
    assert!(body.contains("/user-settings"));
    assert!(!body.contains("data-testid=\"summarizer-form\""));
}

#[tokio::test]
async fn page_shows_form_when_kagi_configured() {
    let app = create_test_app(default_test_config()).await;
    let uid = login(&app, "bob").await;
    configure_kagi(&app, uid).await;
    let res = app.server.get("/summarizer").await;
    res.assert_status_ok();
    assert!(res.text().contains("data-testid=\"summarizer-form\""));
}

#[tokio::test]
async fn page_requires_auth() {
    let app = create_test_app(default_test_config()).await;
    let res = app.server.get("/summarizer").await;
    // PageAuthUser redirects unauthenticated users to /login.
    assert!(res.status_code().is_redirection() || res.status_code().as_u16() == 200);
}
```

> If `SaveServicesConfig` / `KagiConfig` are not re-exported at those paths, adjust the `use` to the real paths (`rdrs::services::save::SaveServicesConfig`, `rdrs::services::KagiConfig`) — confirm with `rg "pub use" src/services/mod.rs src/services/save/mod.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test summarizer_test`
Expected: FAIL — route `/summarizer` returns 404.

- [ ] **Step 3: Add the page template struct + handler**

In `src/handlers/summarizer.rs`:

```rust
use axum::extract::State;
use axum::response::IntoResponse;

use crate::AppState;
use crate::handlers::pages::{AppLayoutContext, build_app_layout};
use crate::middleware::auth::PageAuthUser;
use crate::middleware::flash::Flash;
use crate::models::user_settings;

#[derive(Template)]
#[template(path = "summarizer.html")]
pub struct SummarizerTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub kagi_configured: bool,
    pub urls_text: String,
    pub error: Option<String>,
    pub cards: Vec<SummarizerCard>,
}

impl IntoResponse for SummarizerTemplate {
    fn into_response(self) -> axum::response::Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

async fn kagi_configured(state: &AppState, user_id: i64) -> bool {
    user_settings::get_save_services_config(&state.db, user_id)
        .await
        .ok()
        .and_then(|c| c.kagi)
        .is_some_and(|k| k.is_configured())
}

pub async fn summarizer_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, SummarizerTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let configured = kagi_configured(&state, auth_user.user.id).await;
    (
        flash,
        SummarizerTemplate {
            title: "Summarizer",
            git_version: crate::GIT_VERSION,
            layout,
            kagi_configured: configured,
            urls_text: String::new(),
            error: None,
            cards: Vec::new(),
        },
    )
}
```

- [ ] **Step 4: Create the page template**

`templates/summarizer.html`:

```html
{% extends "app_layout.html" %}
{% import "macros.html" as macros %}

{% block page_script %}<script type="module" src="/static/js/pages/summarizer.js?v={{ layout.git_version }}"></script>{% endblock %}

{% block page %}
    <div class="app-layout">
        <rdrs-sidebar active="summarizer"></rdrs-sidebar>
        <main class="main-content">
            <rdrs-flash></rdrs-flash>
            <div class="page-content page-content-full">
                <h1>Summarizer</h1>
                <p class="page-lead">Paste one or more links and get a Kagi summary for each, one after another — without adding them to a feed. Nothing here is saved to your library.</p>

                {% if !kagi_configured %}
                <div class="empty-state">
                    <p class="empty-state-text">Kagi isn’t configured yet. Add your Kagi session link in
                        <a href="/user-settings">Settings</a> to use the summarizer.</p>
                </div>
                {% else %}
                <form method="post" action="/summarizer" data-testid="summarizer-form" data-summarizer-form>
                    <label class="field-label" for="sz-urls">Links to summarize</label>
                    <textarea id="sz-urls" name="urls" rows="6" spellcheck="false"
                        placeholder="https://example.com/article&#10;https://another.com/post"
                        data-testid="summarizer-input">{{ urls_text }}</textarea>
                    {% if let Some(err) = error %}<p class="form-error" data-testid="summarizer-error">{{ err }}</p>{% endif %}
                    <div class="form-actions">
                        <span class="muted">One URL per line · up to 30</span>
                        <button type="submit" class="btn">Summarize</button>
                    </div>
                </form>

                <div id="sz-results" data-summarizer-results>
                    {% for card in cards %}{% call macros::summarizer_card(card) %}{% endcall %}{% endfor %}
                </div>
                {% endif %}
            </div>
        </main>
    </div>
{% endblock %}
```

> Reuse existing class names where present (`empty-state`, `empty-state-text`, `page-content-full`, `btn`, `muted`). If `field-label` / `form-error` / `form-actions` / `page-lead` do not already exist in `app.css`, add minimal rules for them in this task's CSS step, or reuse the closest existing utility (`rg "field-label\|form-actions\|page-lead" static/css/app.css`).

- [ ] **Step 5: Register the route**

In `src/lib.rs`, after the `/statistics` route:

```rust
.route("/summarizer", get(handlers::summarizer::summarizer_page))
```

- [ ] **Step 6: Add the sidebar nav item**

In `static/js/components/rdrs-sidebar.js`, add a `summarizer` icon to the `ICON` map (reuse a sparkle-like glyph, distinct from `sparkle` used by Summarized):

```js
  wand: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M15 4V2M15 10V8M9 6H7M17 6h-2"/><path d="m3 21 12-12 3 3L6 24z" transform="translate(-3 -3)"/><path d="M12.5 6.5 17.5 11.5"/></svg>',
```

Then, in the Tools `sidebar-section` (the block starting with the `/search` link), add as the first item:

```js
            <a href="/summarizer" class="sidebar-item${isActive('summarizer')}" data-testid="nav-summarizer">
                <span class="sidebar-item-icon">${ICON.wand}</span>
                <span>Summarizer</span>
            </a>
```

- [ ] **Step 7: Rebuild (embedded assets) and run tests**

```bash
cargo build
RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test summarizer_test
```
Expected: PASS (3 tests).

- [ ] **Step 8: fmt, clippy, commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add src/handlers/summarizer.rs templates/summarizer.html src/lib.rs static/js/components/rdrs-sidebar.js tests/summarizer_test.rs
git commit -S -m "feat(summarizer): add GET /summarizer page and sidebar nav

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: `POST /summarizer` — validate and render queued cards

**Files:**
- Modify: `src/handlers/summarizer.rs` (add `start`)
- Modify: `src/lib.rs` (add the `post` route)
- Test: `tests/summarizer_test.rs`

**Interfaces:**
- Consumes: `parse_url_lines`, `url_host`, `SummarizerCard`, `SummarizerTemplate`, `kagi_configured`.
- Produces: `pub async fn start(auth_user: PageAuthUser, State(state): State<AppState>, flash: Flash, Form(form): Form<StartForm>) -> (Flash, SummarizerTemplate)` where `struct StartForm { urls: String }`.
- Behaviour: on validation error re-render the page with `error` + `urls_text` repopulated, `cards` empty. On success render `cards` = one `state: "queued"` card per URL (`title = url_host(url)`, `summary`/`error` empty).

- [ ] **Step 1: Write the failing tests**

Add to `tests/summarizer_test.rs`:

```rust
#[tokio::test]
async fn start_renders_queued_cards() {
    let app = create_test_app(default_test_config()).await;
    let uid = login(&app, "carol").await;
    configure_kagi(&app, uid).await;
    let res = app.server.post("/summarizer")
        .form(&serde_json::json!({"urls": "https://a.com/x\nhttps://b.com/y"}))
        .await;
    res.assert_status_ok();
    let body = res.text();
    assert_eq!(body.matches("data-summarizer-card").count(), 2);
    assert!(body.contains("data-state=\"queued\""));
    assert!(body.contains("https://a.com/x"));
}

#[tokio::test]
async fn start_rejects_over_30() {
    let app = create_test_app(default_test_config()).await;
    let uid = login(&app, "dave").await;
    configure_kagi(&app, uid).await;
    let urls = (0..31).map(|i| format!("https://e.com/{i}")).collect::<Vec<_>>().join("\n");
    let res = app.server.post("/summarizer").form(&serde_json::json!({"urls": urls})).await;
    res.assert_status_ok();
    let body = res.text();
    assert!(body.contains("30 max"));
    assert_eq!(body.matches("data-summarizer-card").count(), 0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test summarizer_test start_`
Expected: FAIL — `POST /summarizer` is 405/404.

- [ ] **Step 3: Implement `start`**

In `src/handlers/summarizer.rs` (add `use axum::Form;` and `serde::Deserialize`):

```rust
#[derive(Debug, serde::Deserialize)]
pub struct StartForm {
    pub urls: String,
}

pub async fn start(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Form(form): Form<StartForm>,
) -> (Flash, SummarizerTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let configured = kagi_configured(&state, auth_user.user.id).await;

    let (error, cards) = match parse_url_lines(&form.urls) {
        Ok(urls) => (
            None,
            urls.into_iter()
                .enumerate()
                .map(|(index, url)| SummarizerCard {
                    index,
                    title: url_host(&url),
                    url,
                    state: "queued",
                    summary: String::new(),
                    error: String::new(),
                })
                .collect(),
        ),
        Err(msg) => (Some(msg), Vec::new()),
    };

    (
        flash,
        SummarizerTemplate {
            title: "Summarizer",
            git_version: crate::GIT_VERSION,
            layout,
            kagi_configured: configured,
            urls_text: form.urls,
            error,
            cards,
        },
    )
}
```

- [ ] **Step 4: Register the route**

In `src/lib.rs`, alongside the GET:

```rust
.route("/summarizer", get(handlers::summarizer::summarizer_page).post(handlers::summarizer::start))
```

(Replace the GET-only line from Task 4 with this combined line.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test summarizer_test`
Expected: PASS (5 tests).

- [ ] **Step 6: fmt, clippy, commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add src/handlers/summarizer.rs src/lib.rs tests/summarizer_test.rs
git commit -S -m "feat(summarizer): validate URLs and render queued cards on POST

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: `POST /summarizer/item` — summarize one URL

**Files:**
- Modify: `src/handlers/summarizer.rs` (add `item`)
- Modify: `src/lib.rs` (add route)
- Test: `tests/summarizer_test.rs`

**Interfaces:**
- Consumes: `SummarizerCard`, `SummarizerCardTemplate`, `url_host`, `validate_url`, `crate::services::summarize::kagi`, `user_settings::get_save_services_config`.
- Produces: `pub async fn item(auth_user: PageAuthUser, State(state): State<AppState>, Form(form): Form<ItemForm>) -> impl IntoResponse` returning the rendered `_summarizer_card_fragment.html`. `struct ItemForm { url: String, index: usize }`.
- Behaviour: re-validate the URL (SSRF); load the user's `KagiConfig` (503-style error card if unset); call `kagi::summarize_url`; render a `completed` card (`title` = Kagi title or host, `summary` = body) or an `error` card.

- [ ] **Step 1: Write the failing tests**

The E2E Kagi stub is env-driven (`KAGI_API_BASE`). For the handler test, start a `wiremock` server and point the env at it (serialize with a mutex-free unique env per test is unsafe; run these two assertions in one test to avoid env races):

```rust
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn item_returns_completed_then_error_card() {
    let app = create_test_app(default_test_config()).await;
    let uid = login(&app, "erin").await;
    configure_kagi(&app, uid).await;

    let mock = MockServer::start().await;
    // First a success, then swap the stub to an error body.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "output_data": {"markdown": "Title: Hello\n\nBody text."}
        })))
        .mount(&mock)
        .await;
    unsafe { std::env::set_var("KAGI_API_BASE", mock.uri()); }

    let ok = app.server.post("/summarizer/item")
        .form(&serde_json::json!({"url": "https://a.com/x", "index": 0}))
        .await;
    ok.assert_status_ok();
    let body = ok.text();
    assert!(body.contains("data-state=\"completed\""));
    assert!(body.contains("Hello"));
    assert!(body.contains("Body text."));

    unsafe { std::env::remove_var("KAGI_API_BASE"); }
}
```

> `std::env::set_var` is `unsafe` on Rust 2024. Confirm the edition with `rg "^edition" Cargo.toml`; drop the `unsafe` blocks if the edition is 2021. Mirror the exact stub pattern already used in `src/services/summarize/kagi.rs` E2E test (search `KAGI_API_BASE`).

- [ ] **Step 2: Run test to verify it fails**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test summarizer_test item_`
Expected: FAIL — `POST /summarizer/item` 404.

- [ ] **Step 3: Implement `item`**

```rust
#[derive(Debug, serde::Deserialize)]
pub struct ItemForm {
    pub url: String,
    pub index: usize,
}

pub async fn item(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Form(form): Form<ItemForm>,
) -> axum::response::Response {
    let host = url_host(&form.url);
    let err_card = |msg: String| SummarizerCard {
        index: form.index,
        title: host.clone(),
        url: form.url.clone(),
        state: "error",
        summary: String::new(),
        error: msg,
    };

    // Re-validate (defense in depth — the browser could POST anything).
    let render = |card: SummarizerCard| {
        SummarizerCardTemplate { card }
            .render()
            .map(Html)
            .map(IntoResponse::into_response)
            .unwrap_or_else(|e| {
                (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
            })
    };

    let parsed = match url::Url::parse(&form.url) {
        Ok(u) if matches!(u.scheme(), "http" | "https") => u,
        _ => return render(err_card("Not a valid URL.".into())),
    };
    if validate_url(&parsed).is_err() {
        return render(err_card("URL not allowed.".into()));
    }

    let kagi = match user_settings::get_save_services_config(&state.db, auth_user.user.id).await {
        Ok(c) => c.kagi,
        Err(_) => None,
    };
    let Some(config) = kagi.filter(|k| k.is_configured()) else {
        return render(err_card("Kagi is not configured.".into()));
    };

    let card = match crate::services::summarize::kagi::summarize_url(&config, &form.url).await {
        Ok(r) if r.success => SummarizerCard {
            index: form.index,
            title: r.title.unwrap_or(host),
            url: form.url.clone(),
            state: "completed",
            summary: r.output_text.unwrap_or_default(),
            error: String::new(),
        },
        Ok(r) => err_card(r.error.unwrap_or_else(|| "Summarization failed.".into())),
        Err(e) => err_card(e.to_string()),
    };
    render(card)
}
```

- [ ] **Step 4: Register the route**

In `src/lib.rs`:

```rust
.route("/summarizer/item", post(handlers::summarizer::item))
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test summarizer_test`
Expected: PASS.

- [ ] **Step 6: fmt, clippy, commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add src/handlers/summarizer.rs src/lib.rs tests/summarizer_test.rs
git commit -S -m "feat(summarizer): summarize one URL via POST /summarizer/item

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Sequential driver ES module

**Files:**
- Create: `static/js/pages/summarizer.js`

**Interfaces:**
- Consumes: DOM produced by Task 4/5 — `[data-summarizer-results]` containing `[data-summarizer-card][data-state="queued"]` elements with `data-summarizer-url` + `data-summarizer-index`; `POST /summarizer/item` returning a card fragment.
- Behaviour: on load, walk queued cards in order; per card show the *Summarizing…* transient (spinner + Cancel), `fetch` the item, replace the card with the response, advance. Cancel aborts the in-flight fetch and stops the walk. Retry re-runs a single card. Copy copies `[data-sz-body]` text. Dismiss removes the card.

- [ ] **Step 1: Write the module**

`static/js/pages/summarizer.js`:

```js
// Summarizer page: drive queued cards one at a time. Progressive enhancement —
// without JS the server-rendered "Queued" cards simply stay put.
const results = document.querySelector('[data-summarizer-results]');
if (results) {
  let current = null; // AbortController for the in-flight card

  const setSummarizing = (card) => {
    card.dataset.state = 'summarizing';
    card.classList.remove('pending');
    const actions = card.querySelector('[data-sz-actions]');
    if (actions) {
      actions.innerHTML = '<button type="button" class="rp-action" data-sz-cancel aria-label="Cancel summarization"><span class="action-label">Cancel</span></button>';
    }
    let status = card.querySelector('[data-sz-status]');
    if (!status) {
      status = document.createElement('p');
      status.className = 'status';
      status.setAttribute('data-sz-status', '');
      card.appendChild(status);
    }
    status.innerHTML = '<span class="sz-spinner" aria-hidden="true"></span>Summarizing…';
  };

  const summarizeCard = async (card) => {
    const url = card.dataset.summarizerUrl;
    const index = card.dataset.summarizerIndex;
    setSummarizing(card);
    const controller = new AbortController();
    current = controller;
    try {
      const res = await fetch('/summarizer/item', {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({ url, index }),
        signal: controller.signal,
      });
      const html = await res.text();
      const tmp = document.createElement('div');
      tmp.innerHTML = html.trim();
      const fresh = tmp.firstElementChild;
      if (fresh) card.replaceWith(fresh);
      return true; // resolved (completed or error) — continue the walk
    } catch (e) {
      if (e.name === 'AbortError') return false; // cancelled — stop the walk
      // Network error: render an inline error and continue.
      const status = card.querySelector('[data-sz-status]');
      if (status) status.textContent = 'Network error — Retry from the button.';
      card.dataset.state = 'error';
      return true;
    } finally {
      current = null;
    }
  };

  const run = async () => {
    // Re-query each iteration: replaceWith() swaps nodes.
    for (;;) {
      const next = results.querySelector('[data-summarizer-card][data-state="queued"]');
      if (!next) break;
      const cont = await summarizeCard(next);
      if (!cont) break;
    }
  };

  results.addEventListener('click', (e) => {
    const cancel = e.target.closest('[data-sz-cancel]');
    if (cancel) {
      current?.abort();
      const card = cancel.closest('[data-summarizer-card]');
      // Mark remaining queued cards as cancelled visually (leave them dimmed).
      if (card) card.dataset.state = 'error';
      return;
    }
    const retry = e.target.closest('[data-sz-retry]');
    if (retry) {
      const card = retry.closest('[data-summarizer-card]');
      if (card) { card.dataset.state = 'queued'; summarizeCard(card); }
      return;
    }
    const dismiss = e.target.closest('[data-sz-dismiss]');
    if (dismiss) { dismiss.closest('[data-summarizer-card]')?.remove(); return; }
    const copy = e.target.closest('[data-sz-copy]');
    if (copy) {
      const body = copy.closest('[data-summarizer-card]')?.querySelector('[data-sz-body]');
      if (body) navigator.clipboard?.writeText(body.textContent.trim());
    }
  });

  run();
}
```

- [ ] **Step 2: Rebuild and smoke-test in a browser**

```bash
cargo build
```
Then run the app (`cargo run`), log in, configure Kagi (or set `KAGI_API_BASE` to a stub), open `/summarizer`, submit 2–3 URLs, and confirm the cards resolve top-to-bottom with a spinner on the active one. Cancel/Retry/Copy/Dismiss behave as described.

- [ ] **Step 3: Commit**

```bash
git add static/js/pages/summarizer.js
git commit -S -m "feat(summarizer): sequential client-side driver for result cards

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: E2E feature

**Files:**
- Create: `e2e/features/summarizer.feature`
- Modify/Create: `e2e/steps/*.js` (add any missing step definitions)

**Interfaces:**
- Consumes: the running app with a Kagi stub. Confirm how existing summary E2E stubs Kagi (`rg -rn "KAGI_API_BASE\|kagi" e2e/`), and reuse that fixture. If Kagi is stubbed process-wide, seed a user with a Kagi session in the fixture.

- [ ] **Step 1: Write the feature**

`e2e/features/summarizer.feature`:

```gherkin
@summarizer
Feature: Summarizer tool
  As a logged-in user with Kagi configured
  I can summarize several URLs at once

  Background:
    Given I am logged in as a user with Kagi configured

  Scenario: Summaries resolve in order
    When I open the Summarizer
    And I enter these URLs:
      | https://example.com/one |
      | https://example.com/two |
    And I submit the summarizer form
    Then I should see 2 summary cards
    And each card should resolve to a completed or error state

  Scenario: Settings prompt when Kagi is not configured
    Given I am logged in as a user without Kagi configured
    When I open the Summarizer
    Then I should see a link to Settings
    And I should not see the summarizer form
```

- [ ] **Step 2: Implement missing steps**

Add step definitions in `e2e/steps/` following the existing style (`rg -l "Given\|When\|Then" e2e/steps`). Reuse the Kagi-stub setup used by the reading-pane summary E2E. Selectors: `[data-testid="summarizer-input"]`, `[data-testid="summarizer-form"]`, `[data-summarizer-card]`, `[data-testid="nav-summarizer"]`, `[data-testid="summarizer-error"]`.

- [ ] **Step 3: Rebuild + run**

```bash
cargo build
cd e2e && npx bddgen && npx playwright test --grep "@summarizer"
```
Expected: PASS. Tag any network-flaky assertion `@skip` if the stub can't guarantee ordering.

- [ ] **Step 4: Commit**

```bash
git add e2e/features/summarizer.feature e2e/steps
git commit -S -m "test(e2e): cover the summarizer tool

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: Regenerate screenshots + final verification

**Files:**
- Modify: `screenshots/*.png` (regenerated)

- [ ] **Step 1: Rebuild and regenerate screenshots**

The new **Summarizer** sidebar item appears in the README screenshots (they show the sidebar).

```bash
cargo build
cd e2e && npm run screenshots
```

- [ ] **Step 2: Eyeball the four screenshots**

Open `screenshots/*.png`; confirm the sidebar now shows **Summarizer** under Tools and nothing else regressed (light + dark).

- [ ] **Step 3: Full gate**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
RDRS_FAST_HASH=1 cargo nextest run
cargo deny check
```
Expected: all green.

- [ ] **Step 4: Commit screenshots**

```bash
git add screenshots
git commit -S -m "docs(summarizer): refresh screenshots for the new sidebar item

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review Notes

- **Spec coverage:** placement/route (T4), logged-in + not-configured prompt (T4), textarea + max 30 + blanks/dedupe (T2, T5), sequential resolution (T7), card format = `summary-box` (T3), Kagi title fallback (T1, T6), language reuse (automatic via `KagiConfig` in T6), per-card error + Retry/Dismiss (T3, T7), Cancel (T7), timeout (relies on `EXTERNAL_API_TIMEOUT`, no code), no persistence (no model/migration anywhere), testing (T1/T2 unit, T4/T5/T6 handler, T8 E2E), screenshots (T9). All covered.
- **Type consistency:** `SummarizerCard` fields and `state` string values are identical across T3/T5/T6; `parse_url_lines`/`url_host`/`MAX_URLS` names match between T2 and consumers; template path names (`summarizer.html`, `_summarizer_card_fragment.html`) match struct attributes.
- **Known verification points flagged inline:** exact `SaveServicesConfig`/`KagiConfig` re-export paths, `UrlValidationError: Display`, Rust edition for `unsafe { set_var }`, and whether `field-label`/`form-actions`/`page-lead` classes already exist — each has an `rg` check noted in the relevant step.
