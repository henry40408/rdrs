use maud::{Markup, PreEscaped, html};

use super::{icons, layout, macros};
use crate::handlers::pages::{
    SearchFragmentTemplate, SearchRefreshFragmentTemplate, SearchResultView, SearchResultsView,
    SearchTemplate,
};

pub(crate) fn page(page: &SearchTemplate) -> Markup {
    let layout = &page.layout;
    let script = layout::module_script(layout.git_version, "search.js");
    let body = html! {
        div class="app-layout" {
            rdrs-sidebar active="search" {}
            main class="main-content" {
                (macros::flash_mount(&layout.flash_messages))
                div class="page-content page-content-full" {
                    h1 { "Search" }
                    (search_form(&page.search.q))
                    (syntax_popover())
                    div class="search-results-region" data-search-results {
                        (results(&page.search))
                    }
                }
            }
        }
    };
    layout::app_page(layout, page.title, Some(script), body)
}

/// Live-search response: replaces the results, leaving the form (and the
/// focused input) untouched.
pub(crate) fn refresh_fragment(fragment: &SearchRefreshFragmentTemplate) -> Markup {
    html! {
        template data-swap-target="[data-search-results]" {
            div class="search-results-region" data-search-results {
                (results(&fragment.search))
            }
        }
    }
}

/// Load More response: the items (and the next Load More, if any) are
/// inserted before the old `#search-load-more`, which is then removed.
pub(crate) fn fragment(fragment: &SearchFragmentTemplate) -> Markup {
    html! {
        template data-swap-target="#search-load-more" {
            @for result in &fragment.search.results {
                (result_item(result))
            }
            (load_more(&fragment.search))
        }
    }
}

/// Outside the swapped results, so typing keeps focus and caret. No
/// `required`: clearing the field searches too, back to the empty state.
fn search_form(q: &str) -> Markup {
    html! {
        form method="get" action="/search" data-swap="[data-search-results]" data-swap-fragment data-swap-latest data-sync-q data-live-search="300" data-live-search-min="2" {
            div class="search-form-row" {
                input type="text" id="search-q" name="q" value=(q) placeholder="Find entries by title or content" aria-label="Search keywords" autocomplete="off" autofocus data-testid="search-input";
                " "
                button type="button" class="filter-chip-btn" data-search-help popovertarget="search-syntax-popover" data-testid="search-help" aria-label="Search syntax" title="Search syntax" { (icons::help()) }
                " "
                button type="submit" data-testid="search-btn" { "Search" }
            }
        }
    }
}

/// No-JS fallback for the help button: with scripts, app.js cancels the
/// click and opens `<rdrs-kb-help>` instead. Keep in step with `SEARCH_SYNTAX`.
fn syntax_popover() -> Markup {
    html! {
        div id="search-syntax-popover" class="search-syntax-popover" popover data-testid="search-syntax-popover" {
            h2 { "Search syntax" }
            dl {
                dt { code { "is:unread" } " " code { "is:read" } " " code { "is:starred" } }
                dd { "Status" }
                dt { code { "feed:name" } " " code { "category:name" } }
                dd { "By feed or category (fuzzy, case-insensitive)" }
                dt { code { "title:word" } " " code { "author:name" } }
                dd { "Title contains word; by author" }
                dt { code { "before:2026-01-01" } " " code { "after:2026-01-01" } }
                dd { "Published before / after (UTC)" }
                dt { code { "AND OR NOT" } " " code { "( )" } }
                dd { "Boolean operators and grouping; adjacent words imply AND" }
                dt { code { "-term" } " " code { "\"exact phrase\"" } }
                dd { "Exclude a term; quote phrases and values with spaces, e.g. " code { "feed:\"Rust Blog\"" } }
            }
        }
    }
}

fn results(search: &SearchResultsView) -> Markup {
    if let Some(err) = &search.error {
        return html! {
            div class="search-error" role="alert" data-testid="search-error" { (err) }
        };
    }
    if search.q.is_empty() {
        return html! {
            div class="empty-state" {
                h2 class="empty-state-title" { "Search your library" }
                p class="empty-state-text" { "Type a keyword and press " kbd class="empty-state-kbd" { "Enter" } " to find entries by title or content." }
            }
        };
    }
    if search.results.is_empty() {
        return html! {
            div class="empty-state" data-testid="search-empty" {
                h2 class="empty-state-title" { "No matches" }
                p class="empty-state-text" { "Nothing matched “" (search.q) "”. Try another keyword or check the spelling." }
            }
        };
    }
    html! {
        ul class="search-results" data-testid="search-results" {
            @for result in &search.results {
                (result_item(result))
            }
            (load_more(search))
        }
    }
}

/// `title_html` and `snippet_html` come from `highlight_html`, which escapes
/// all text and adds only `<mark>` tags, so they are safe to emit raw.
fn result_item(result: &SearchResultView) -> Markup {
    html! {
        li class="search-result" {
            a href={ "/entries/" (result.entry_id) } class="search-result-title" { (PreEscaped(&result.title_html)) }
            div class="search-result-meta" {
                span class="muted" { (result.feed_title) }
                @if !result.published_relative.is_empty() {
                    " "
                    span class="muted" {
                        "· "
                        @if result.published_at_iso.is_empty() {
                            (result.published_relative)
                        } @else {
                            time datetime=(result.published_at_iso) { (result.published_relative) }
                        }
                        " "
                    }
                }
            }
            @if !result.snippet_html.is_empty() {
                p class="search-result-snippet" { (PreEscaped(&result.snippet_html)) }
            }
        }
    }
}

/// The last item of the results list, so a Load More swap inserts the next
/// page's items inside the `<ul>`. JS adds `fragment=1` (`data-swap-fragment`);
/// without it the form navigates to the next page as a whole document.
fn load_more(search: &SearchResultsView) -> Markup {
    let Some(after) = &search.next_cursor else {
        return html! {};
    };
    html! {
        li id="search-load-more" class="search-load-more" {
            form method="get" action="/search" data-swap="#search-load-more" data-swap-fragment class="load-more-form" {
                input type="hidden" name="q" value=(search.q);
                " "
                input type="hidden" name="after" value=(after);
                " "
                button type="submit" data-testid="search-load-more-btn" { "Load More" }
            }
        }
    }
}
