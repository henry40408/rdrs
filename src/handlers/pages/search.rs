use askama::Template;
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Response},
};

use super::search_text::{build_snippet, highlight_html};
use super::{
    AppLayoutContext, build_app_layout, fetch_entries_page, format_relative_time,
    search_error_message,
};
use crate::AppState;
use crate::middleware::auth::PageAuthUser;
use crate::middleware::flash::Flash;
use crate::models::entry;

#[derive(serde::Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    /// Load More cursor; see [`entry::ContinuationCursor`].
    pub after: Option<String>,
    /// `1` from the swap helper: answer with a fragment, not the page.
    pub fragment: Option<u8>,
}

/// Results per `/search` page. Fixed rather than the entries-per-page setting:
/// reading that costs a query on every keystroke of search-as-you-type.
const SEARCH_PAGE_SIZE: i64 = 50;

/// `GET /search` — newest matches, a page at a time; empty `q` shows the form.
/// `?fragment=1` answers live search (whole results) or Load More (`after`) with
/// a fragment, except to a document navigation, which always gets the page.
pub async fn search_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    headers: axum::http::HeaderMap,
    Query(query): Query<SearchQuery>,
) -> Response {
    let q = query.q.unwrap_or_default().trim().to_string();
    let user_id = auth_user.user.id;
    let cursor = query
        .after
        .as_deref()
        .filter(|s| !s.is_empty())
        .and_then(entry::ContinuationCursor::parse);
    let search = search_results(&state, user_id, q, SEARCH_PAGE_SIZE, cursor).await;

    if query.fragment == Some(1) && !crate::handlers::is_document_navigation(&headers) {
        return if query.after.is_some() {
            (flash, SearchFragmentTemplate { search }).into_response()
        } else {
            (flash, SearchRefreshFragmentTemplate { search }).into_response()
        };
    }

    let layout = build_app_layout(&state, &auth_user, &flash).await;
    (
        flash,
        SearchTemplate {
            title: "Search",
            git_version: crate::GIT_VERSION,
            layout,
            search,
        },
    )
        .into_response()
}

/// One page of `/search` results for `q`, highlighted.
async fn search_results(
    state: &AppState,
    user_id: i64,
    q: String,
    page_size: i64,
    cursor: Option<entry::ContinuationCursor>,
) -> SearchResultsView {
    let mut view = SearchResultsView {
        q,
        error: None,
        results: Vec::new(),
        next_cursor: None,
    };
    if view.q.is_empty() {
        return view;
    }
    let ast = match entry::query::parse(&view.q) {
        Ok(ast) => ast,
        Err(e) => {
            view.error = Some(search_error_message(&view.q, &e));
            return view;
        }
    };
    let terms = entry::query::free_text_terms(&ast);
    let needles: Vec<&str> = terms.iter().map(String::as_str).collect();
    let filter = entry::EntryFilter {
        query: Some(ast),
        ..Default::default()
    };
    let (rows, next_cursor) = fetch_entries_page(
        state,
        user_id,
        &filter,
        entry::EntrySortOrder::PublishedAt,
        page_size,
        cursor,
    )
    .await
    .unwrap_or_default();
    view.next_cursor = next_cursor;
    view.results = rows
        .into_iter()
        .map(|e| {
            let title = e
                .entry
                .title
                .clone()
                .unwrap_or_else(|| "(no title)".to_string());
            let snippet = build_snippet(
                e.entry.content.as_deref().or(e.entry.summary.as_deref()),
                &needles,
                200,
            );
            let (published_relative, published_at_iso) = format_relative_time(e.entry.published_at);
            SearchResultView {
                entry_id: e.entry.id,
                title_html: highlight_html(&title, &needles),
                feed_title: e.feed_title.clone().unwrap_or_else(|| e.feed_url.clone()),
                published_relative,
                published_at_iso,
                snippet_html: highlight_html(&snippet, &needles),
            }
        })
        .collect();
    view
}

/// `/search` result row. `title_html`/`snippet_html` are pre-escaped with
/// `<mark>` highlights; render with `|safe`.
pub struct SearchResultView {
    pub entry_id: i64,
    pub title_html: String,
    pub feed_title: String,
    pub published_relative: String,
    /// RFC 3339, or empty when unknown.
    pub published_at_iso: String,
    pub snippet_html: String,
}

/// What `_search_results.html` renders, shared by the page and its fragments.
pub struct SearchResultsView {
    pub q: String,
    pub error: Option<String>,
    pub results: Vec<SearchResultView>,
    /// Load More cursor, when another page exists.
    pub next_cursor: Option<String>,
}

/// Per-route template for `/search`.
#[derive(Template)]
#[template(path = "search.html")]
pub struct SearchTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub search: SearchResultsView,
}

/// Live-search response: the whole results region.
#[derive(Template)]
#[template(path = "_search_refresh_fragment.html")]
pub struct SearchRefreshFragmentTemplate {
    pub search: SearchResultsView,
}

/// Search Load More response: the next page's items.
#[derive(Template)]
#[template(path = "_search_fragment.html")]
pub struct SearchFragmentTemplate {
    pub search: SearchResultsView,
}
