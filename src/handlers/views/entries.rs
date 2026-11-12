//! Entry list pages and their swap fragments.

use maud::{Markup, html};

use super::{icons, layout, macros, reading};
use crate::handlers::pages::{
    EntriesFragmentTemplate, EntriesLayoutContext, EntriesPageTemplate,
    EntriesPaneFragmentTemplate, EntriesRefreshFragmentTemplate, EntryRowView,
};

/// What the left column and its list body need; shared by the full page and
/// the category-swap fragment so the two cannot drift.
struct ListPane<'a> {
    title: &'a str,
    entries: &'a [EntryRowView],
    next_cursor: Option<&'a str>,
    layout: &'a EntriesLayoutContext,
    csrf_token: &'a str,
}

/// Every entry-list page.
pub(crate) fn page(template: &EntriesPageTemplate) -> Markup {
    let list = ListPane {
        title: &template.title,
        entries: &template.entries,
        next_cursor: template.next_cursor.as_deref(),
        layout: &template.entries_layout,
        csrf_token: &template.csrf_token,
    };
    let body = html! {
        div.app-layout {
            (sidebar(&template.entries_layout))
            main.main-content {
                (macros::flash_mount(&template.layout.flash_messages))
                div.split-view {
                    (list_pane(&list))
                    @if let Some(pane) = &template.reading_pane {
                        (reading::pane(pane, &template.csrf_token))
                    } @else {
                        (reading::empty_pane())
                    }
                }
            }
        }
    };
    layout::app_page(&template.layout, &template.title, None, body)
}

fn sidebar(layout: &EntriesLayoutContext) -> Markup {
    html! {
        rdrs-sidebar active=(layout.active) active-category-id=[layout.active_category_id] active-feed-id=[layout.active_feed_id] {}
    }
}

/// Load More response: the template's children are inserted before the old
/// `#load-more`, which is then removed, so earlier rows and scroll stay put.
pub(crate) fn load_more_fragment(fragment: &EntriesFragmentTemplate) -> Markup {
    html! {
        template data-swap-target="#load-more" {
            @for row in &fragment.entries { (entry_row(row, &fragment.csrf_token)) }
            @if let Some(after) = &fragment.next_cursor {
                // Echoed, never re-stamped: every page of one session paginates against the same boundary.
                (load_more_form(&LoadMore {
                    path: &fragment.path,
                    after,
                    snapshot: fragment.snapshot.as_deref(),
                    status: fragment.status_filter.as_deref(),
                    query: fragment.q.as_deref(),
                }))
            }
        }
    }
}

/// Category-switch response (`?pane=1`): replaces the left column and resets
/// the reading pane, leaving the sidebar and its scroll position untouched.
pub(crate) fn pane_fragment(fragment: &EntriesPaneFragmentTemplate) -> Markup {
    let list = ListPane {
        title: &fragment.title,
        entries: &fragment.entries,
        next_cursor: fragment.next_cursor.as_deref(),
        layout: &fragment.entries_layout,
        csrf_token: &fragment.csrf_token,
    };
    html! {
        template data-swap-target="[data-list-pane]" { (list_pane(&list)) }
        template data-swap-target="#reading-pane" { (reading::empty_pane()) }
    }
}

/// Scoped-search response: replaces the entries list and the mark-matching
/// slot, leaving the header (and the focused search input) untouched.
pub(crate) fn refresh_fragment(fragment: &EntriesRefreshFragmentTemplate) -> Markup {
    let layout = &fragment.entries_layout;
    let csrf = fragment.csrf_token.as_str();
    let list = ListPane {
        title: "",
        entries: &fragment.entries,
        next_cursor: fragment.next_cursor.as_deref(),
        layout,
        csrf_token: csrf,
    };
    html! {
        template data-swap-target="[data-entries-list]" { (entries_list(&list)) }
        template data-swap-target="[data-search-error]" { (search_error(layout)) }
        template data-swap-target="[data-mark-all-slot]" { (mark_all_slot(layout, csrf)) }
        template data-swap-target="[data-mark-matching-slot]" { (mark_matching_slot(layout, csrf)) }
    }
}

fn mark_all_slot(layout: &EntriesLayoutContext, csrf: &str) -> Markup {
    html! { span.mark-all-slot data-mark-all-slot { (mark_all_button(layout, csrf)) } }
}

fn mark_matching_slot(layout: &EntriesLayoutContext, csrf: &str) -> Markup {
    html! { span.mark-matching-slot data-mark-matching-slot { (mark_matching_button(layout, csrf)) } }
}

/// Always rendered so a swap can fill it; `hidden` while the query parses.
fn search_error(layout: &EntriesLayoutContext) -> Markup {
    html! {
        div.search-error.search-drawer-error role="alert" data-search-error data-testid="scoped-search-error" hidden[layout.search_error.is_none()] {
            @if let Some(err) = &layout.search_error { (err) }
        }
    }
}

fn entries_list(list: &ListPane) -> Markup {
    html! {
        div.list-pane-body data-entries-list data-snapshot-at=(list.layout.snapshot_at) {
            (list_body(list))
        }
    }
}

/// The whole left column.
fn list_pane(list: &ListPane) -> Markup {
    let layout = list.layout;
    html! {
        div.list-pane data-list-pane {
            @if let Some(action) = &layout.search_action { (search_drawer(layout, action)) }
            div.list-pane-header {
                (breadcrumb(layout))
                (title_row(list))
                @if layout.show_tab_bar { (tab_bar(layout.active)) }
                @if layout.filter_tabs.is_some() || layout.mark_as_read_scope.is_some() || layout.mark_all_action.is_some() {
                    (filter_bar(layout, list.csrf_token))
                }
            }
            (entries_list(list))
        }
    }
}

/// Search drawer rather than inline (it cost a whole row at <=1024px).
/// Rendered open when `q` is present so deep links and swaps don't flash.
fn search_drawer(layout: &EntriesLayoutContext, action: &str) -> Markup {
    html! {
        div #search-drawer.search-drawer.is-open[layout.search.is_some()] data-search-drawer {
            div.search-drawer-clip {
                div.search-drawer-row {
                    form.entries-search method="get" action=(action) data-swap="[data-entries-list]" data-swap-fragment data-swap-latest data-sync-q data-live-search="250" data-entries-search {
                        @if let Some(status) = &layout.status_filter { input type="hidden" name="status" value=(status); }
                        " "
                        input type="search" name="q" class="search-input" placeholder="Search in this view…" autocomplete="off" data-testid="scoped-search-input" value=[layout.search.as_deref()];
                    }
                    button.filter-chip-btn type="button" data-search-help data-testid="scoped-search-help" aria-label="Search syntax" title="Search syntax" { (icons::help()) }
                    " "
                    button.filter-chip-btn type="button" data-search-close data-testid="scoped-search-close" aria-label="Close search" { (icons::close()) }
                }
                (search_error(layout))
            }
        }
    }
}

fn breadcrumb(layout: &EntriesLayoutContext) -> Markup {
    html! {
        @if !layout.breadcrumb_items.is_empty() {
            nav.breadcrumb data-testid="breadcrumb" {
                @for (index, item) in layout.breadcrumb_items.iter().enumerate() {
                    @if index > 0 { span.breadcrumb-sep { " / " } }
                    @if let Some(href) = &item.href {
                        a href=(href) { (item.label) }
                    } @else {
                        span { (item.label) }
                    }
                }
            }
        }
    }
}

/// The search toggle sits beside the title, not in the filter bar: views
/// without selects (e.g. Read) would otherwise get a row holding only it.
fn title_row(list: &ListPane) -> Markup {
    let layout = list.layout;
    html! {
        div.list-pane-title-row {
            h1 {
                @if let Some(feed_id) = layout.header_feed_icon_id {
                    img.feed-icon src={ "/api/feeds/" (feed_id) "/icon" } alt="" width="20" height="20" decoding="sync";
                }
                (list.title)
            }
            @if layout.search_action.is_some() {
                button.filter-chip-btn type="button" data-search-toggle data-testid="scoped-search-toggle" aria-controls="search-drawer" aria-expanded=(if layout.search.is_some() { "true" } else { "false" }) aria-label="Search in this view" { (icons::search()) }
            }
        }
    }
}

fn tab_bar(active: &str) -> Markup {
    let tabs = [
        ("all", "/entries", "All"),
        ("read", "/entries/read", "Read"),
        ("starred", "/entries/starred", "Starred"),
        ("summarized", "/entries/summarized", "Summarized"),
    ];
    html! {
        div.tab-bar {
            @for (index, (key, href, label)) in tabs.into_iter().enumerate() {
                @if index > 0 { " " }
                a class=[(active == key).then_some("active")] href=(href) data-testid={ "tab-" (key) } data-tab=(key) { (label) }
            }
        }
    }
}

fn filter_bar(layout: &EntriesLayoutContext, csrf: &str) -> Markup {
    html! {
        div.filter-bar {
            @if let Some(tabs) = &layout.filter_tabs {
                div.form-group.form-group-inline data-status-filter {
                    select #status-filter data-testid="status-filter-select" data-status-select class="select-auto" {
                        @for tab in tabs { option value=(tab.href) selected[tab.active] { (tab.label) } }
                    }
                }
            }
            @if let Some(scope) = &layout.mark_as_read_scope {
                div.form-group.form-group-inline {
                    select #mark-read-age data-testid="mark-read-select" class="select-auto" data-mark-read-scope=(scope) {
                        option value="" { "Mark as Read..." }
                        option value="1" { "Older than 1 day" }
                        option value="7" { "Older than 1 week" }
                        option value="30" { "Older than 1 month" }
                        option value="365" { "Older than 1 year" }
                        option value="all" { "All entries" }
                    }
                }
            }
            @if layout.mark_all_action.is_some() {
                (mark_all_slot(layout, csrf))
                " "
            }
            @if layout.search_action.is_some() { (mark_matching_slot(layout, csrf)) }
        }
    }
}

/// Hidden while searching: it would mark rows the list does not show.
/// `data-snapshot-from-list` refreshes the field at submit, after any in-place
/// list refresh.
fn mark_all_button(layout: &EntriesLayoutContext, csrf: &str) -> Markup {
    let Some(action) = &layout.mark_all_action else {
        return html! {};
    };
    if layout.search.is_some() {
        return html! {};
    }
    html! {
        form.form-group.form-group-inline method="post" action=(action) data-confirm="Mark all of today's unread entries as read?" data-snapshot-from-list data-testid="mark-all-form" {
            (macros::csrf_field(csrf))
            " "
            input type="hidden" name="snapshot" value=(layout.snapshot_at);
            " "
            button.btn.btn-secondary type="submit" data-testid="mark-all-btn" { "Mark all as Read" }
        }
    }
}

fn mark_matching_button(layout: &EntriesLayoutContext, csrf: &str) -> Markup {
    let Some(count) = layout.matching_count.filter(|n| *n > 0) else {
        return html! {};
    };
    let Some(action) = &layout.mark_matching_action else {
        return html! {};
    };
    html! {
        form.form-group.form-group-inline method="post" action=(action) data-testid="mark-matching-form" data-confirm={ "Mark " (count) " matching entries as read?" } data-snapshot-from-list {
            (macros::csrf_field(csrf))
            " "
            @if let Some(status) = &layout.status_filter { input type="hidden" name="status" value=(status); }
            " "
            input type="hidden" name="snapshot" value=(layout.snapshot_at);
            " "
            @if let Some(query) = &layout.search { input type="hidden" name="q" value=(query); }
            " "
            button.btn.btn-secondary type="submit" data-testid="mark-matching-btn" { "Mark " (count) " matching as Read" }
        }
    }
}

fn list_body(list: &ListPane) -> Markup {
    if list.entries.is_empty() {
        return empty_state(list.layout);
    }
    let layout = list.layout;
    html! {
        @for row in list.entries { (entry_row(row, list.csrf_token)) }
        @if let Some(after) = list.next_cursor {
            // The boundary keeps entries read meanwhile from vanishing from the next page.
            (load_more_form(&LoadMore {
                path: &layout.path,
                after,
                snapshot: Some(&layout.snapshot_at),
                status: layout.status_filter.as_deref(),
                query: layout.search.as_deref(),
            }))
        }
        @if layout.show_mark_above {
            div.mark-above-form {
                button #mark-above-read.btn.btn-secondary type="button" data-testid="mark-above-btn" { "Mark Above as Read" }
            }
        }
    }
}

fn empty_state(layout: &EntriesLayoutContext) -> Markup {
    if layout.onboarding {
        return onboarding_guide();
    }
    // The error banner above the list says why; nothing to list.
    if layout.search_error.is_some() {
        return html! {};
    }
    if layout.search.is_some() {
        return html! {
            div.empty-state-quiet data-testid="scoped-search-empty" {
                p.empty-state-quiet-title { "No matches" }
                p.empty-state-quiet-text { "Nothing in this view matches your search." }
            }
        };
    }
    html! {
        div.empty-state-quiet {
            p.empty-state-quiet-title { (layout.empty_title) }
            p.empty-state-quiet-text { (layout.empty_detail) }
        }
    }
}

fn onboarding_guide() -> Markup {
    html! {
        div.empty-state-quiet.onboarding-guide data-testid="onboarding-guide" {
            p.empty-state-quiet-title { "Welcome to rdrs" }
            p.empty-state-quiet-text { "Three steps to your first read:" }
            ol.onboarding-steps {
                li { "Add a feed, or import an OPML file" }
                li { "Wait for the first sync to fetch entries" }
                li { "Read" }
            }
            div.onboarding-cta {
                a.btn href="/feeds" data-testid="onboarding-add-feed" { "Add your first feed →" }
                " "
                a.btn.btn-secondary href="/feeds/import" data-testid="onboarding-import-opml" { "Import OPML" }
            }
        }
    }
}

struct LoadMore<'a> {
    path: &'a str,
    after: &'a str,
    snapshot: Option<&'a str>,
    status: Option<&'a str>,
    query: Option<&'a str>,
}

fn load_more_form(load_more: &LoadMore) -> Markup {
    html! {
        form #load-more.load-more-form method="get" action=(load_more.path) data-swap="#load-more" {
            input type="hidden" name="after" value=(load_more.after);
            " "
            input type="hidden" name="fragment" value="1";
            " "
            @if let Some(snapshot) = load_more.snapshot { input type="hidden" name="snapshot" value=(snapshot); }
            " "
            @if let Some(status) = load_more.status { input type="hidden" name="status" value=(status); }
            " "
            @if let Some(query) = load_more.query { input type="hidden" name="q" value=(query); }
            " "
            button type="submit" data-testid="load-more-btn" { "Load More" }
        }
    }
}

/// Ids, data-* attributes, data-testids and the `.entry-status` / `.entry-read`
/// / `.selected` hooks are relied on by app.js, SSE and e2e.
pub(crate) fn entry_row(row: &EntryRowView, csrf: &str) -> Markup {
    html! {
        div.entry-item.entry-read[row.is_read] id={ "entry-row-" (row.id) } data-entry-row data-entry-id=(row.id) data-category-id=(row.category_id) data-entry-link=[row.link.as_deref()] data-testid="entry-item" {
            // Marker: dot = idempotent read/unread toggle (mirrors the star form).
            (macros::row_marker_form(row.id, row.is_read, csrf))
            // `lang` keeps a Simplified title on one font instead of mixing PingFang TC/SC. See `utils::han`.
            a.entry-item-title href={ "/entries/" (row.id) "/fragment" } data-swap="#reading-pane" title=(row.title) data-testid="entry-title-link" lang=[row.title_lang()] { (row.title) }
            " "
            span.entry-timerow {
                " "
                (entry_status(row))
                " "
                time.entry-time datetime=(row.published_at_iso) { (row.published_relative) }
                " "
            }
            div.entry-item-meta {
                (row_favicon(row))
                " "
                span.entry-meta-text { a.entry-feed href={ "/feeds/" (row.feed_id) "/entries" } { (row.feed_title) } }
            }
            div.rail-actions {
                (macros::row_star_form(row.id, row.is_starred, csrf))
                @if let Some(link) = &row.link {
                    a.entry-open-ext href=(link) target="_blank" rel="noopener noreferrer" data-testid="entry-open-original" aria-label="Open original" title="Open original (v)" { (icons::external()) }
                }
            }
        }
    }
}

/// Must render truly empty with no badge so `.entry-status:empty` hides it;
/// SSE swaps this node.
fn entry_status(row: &EntryRowView) -> Markup {
    html! {
        span.entry-status {
            @match row.summary_status_str() {
                Some("completed") => { span.summary-badge title="Has Summary" aria-hidden="true" { (icons::summary(true)) } }
                Some("pending") => { span.summary-badge-pending title="Pending" aria-hidden="true" { (icons::summary(false)) } }
                Some("processing") => { span.summary-badge-processing title="Processing" aria-hidden="true" { (icons::summary(false)) } }
                Some("failed") => { span.summary-badge-failed title="Failed" aria-hidden="true" { (icons::summary(true)) } }
                _ => {}
            }
        }
    }
}

/// No `loading="lazy"` and sync decoding: `WebKit` otherwise blinks these icons
/// on iOS whenever the row is re-inserted or repainted.
fn row_favicon(row: &EntryRowView) -> Markup {
    html! {
        @if row.feed_has_icon {
            img.entry-favicon src={ "/api/feeds/" (row.feed_id) "/icon" } alt="" decoding="sync" width="15" height="15";
        } @else {
            span class={ "entry-favicon entry-favicon-chip fav-c" (row.feed_color_index()) } aria-hidden="true" { (row.feed_initial()) }
        }
    }
}
