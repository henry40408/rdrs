use maud::{Markup, html};

use super::{layout, macros};
use crate::handlers::pages::{
    FeedCategoryOption, FeedEditTemplate, FeedFilterLink, FeedRowView, FeedsImportTemplate,
    FeedsTemplate,
};

const ERROR_ROW_CLASS: &str = "feed-error-no-border";

pub(crate) fn page(page: &FeedsTemplate) -> Markup {
    let layout = &page.layout;
    let body = html! {
        div class="app-layout" {
            rdrs-sidebar active="feeds" {}
            main class="main-content" {
                (macros::flash_mount(&layout.flash_messages))
                div class="page-content" {
                    h1 { "Feeds" }
                    div class="feeds-toolbar" {
                        a href="/reader/api/0/subscription/export" class="btn btn-secondary" { "Export OPML" }
                        " "
                        a href="/feeds/import" class="btn btn-secondary" { "Import OPML" }
                    }
                    (add_feed_form(page))
                    (filter_bar(page))
                    (freshness_help(page))
                    (feeds_table(page))
                }
            }
        }
    };
    layout::app_page(layout, page.title, None, body)
}

fn add_feed_form(page: &FeedsTemplate) -> Markup {
    html! {
        form method="post" action="/feeds" {
            (macros::csrf_field(&page.csrf_token))
            " "
            input type="hidden" name="return_to" value=(page.return_to);
            " "
            div class="form-group" {
                label for="url" { "Feed URL" }
                " "
                input type="text" id="url" name="url" placeholder="https://example.com/feed.xml or https://example.com" required data-testid="feed-url-input";
            }
            div class="form-group" {
                label for="category_id" { "Category" }
                " "
                select id="category_id" name="category_id" required data-testid="feed-category-select" {
                    @if page.categories.is_empty() {
                        option value="" { "No categories available" }
                    } @else {
                        @for c in &page.categories {
                            option value=(c.id) { (c.name) }
                        }
                    }
                }
            }
            button type="submit" data-testid="add-feed-btn" { "Add Feed" }
        }
    }
}

fn filter_bar(page: &FeedsTemplate) -> Markup {
    html! {
        form method="get" action="/feeds" class="filter-bar" {
            div class="form-group form-group-inline" {
                label for="filter-category" { "Category" }
                " "
                select id="filter-category" name="category" class="select-auto" data-submit-on-change {
                    option value="" { "All Categories (" (page.total_feed_count) ")" }
                    @for c in &page.categories {
                        option value=(c.id) selected[page.active_category_id == Some(c.id)] { (c.name) " (" (c.feed_count) ")" }
                    }
                }
            }
            div class="form-group form-group-inline" {
                label for="sort-by" { "Sort" }
                " "
                select id="sort-by" name="sort" class="select-auto" data-submit-on-change {
                    option value="title" selected[page.active_sort == "title"] { "Title" }
                    option value="unread" selected[page.active_sort == "unread"] { "Unread Count" }
                    option value="category" selected[page.active_sort == "category"] { "Category" }
                    @if page.open_rate_shown {
                        option value="open_rate" selected[page.active_sort == "open_rate"] { "Open Rate (lowest first)" }
                    }
                }
            }
            div class="form-group form-group-inline feed-filter-links" {
                @for link in &page.filter_links {
                    (filter_link(link))
                    " "
                }
            }
            input type="hidden" name="filter" value=(page.active_filter);
            " "
            // No-JS submit button: the selects otherwise rely on `data-submit-on-change`.
            noscript {
                div class="form-group form-group-inline" {
                    button type="submit" class="btn btn-secondary" data-testid="feed-filter-apply" { "Apply" }
                }
            }
        }
    }
}

fn filter_link(link: &FeedFilterLink) -> Markup {
    html! {
        a href=(link.href) class={ "feed-filter-link" @if link.active { " active" } } { (link.label) }
    }
}

fn freshness_help(page: &FeedsTemplate) -> Markup {
    html! {
        details class="feed-freshness-help" data-testid="feed-freshness-help" {
            summary { "How “Fetched” and “Updated” are decided" }
            div class="feed-freshness-help-body" {
                ul {
                    li { strong { "Fetched" } " — when rdrs last " em { "tried" } " to read this feed, whether or not it worked. Every feed is retried about once an hour." }
                    li { strong { "Updated" } " — the " em { "newest" } " of three signals, whichever of them this feed provides. They are not ranked: a missing signal is skipped, not treated as old." }
                    li {
                        "Those signals are the feed’s own date (" code { "<updated>" } ", falling back to " code { "<pubDate>" } " / " code { "<lastBuildDate>" } "), its newest entry’s date (" code { "<published>" } ", falling back to that entry’s " code { "<updated>" } "), and the " code { "Last-Modified" } " header its server sends."
                    }
                    li { "rdrs re-asks with " code { "If-None-Match" } " / " code { "If-Modified-Since" } ". A " code { "304 Not Modified" } " moves " strong { "Fetched" } " forward and leaves " strong { "Updated" } " untouched." }
                    li { "A feed that publishes no dates at all shows " em { "No date info" } "; its age is then judged by " strong { "Fetched" } " instead." }
                    li {
                        "Age: up to " (page.fresh_max_days) " days is normal, "
                        span class="feed-freshness-warning" { (page.fresh_max_days + 1) "–" (page.warning_max_days) " days" }
                        " is a warning, "
                        span class="feed-freshness-stale" { "over " (page.warning_max_days) " days" }
                        " — or no date at all — is stale. The " strong { "Stale" } " filter matches only that last group."
                    }
                    li { "A failed fetch shows its error under the row and counts towards the " strong { "Errors" } " filter; it clears as soon as one fetch succeeds. Feeds are never auto-disabled for failing." }
                }
            }
        }
    }
}

fn feeds_table(page: &FeedsTemplate) -> Markup {
    let colspan = if page.open_rate_shown { 5 } else { 4 };
    html! {
        table class="mobile-cards feeds-table" {
            thead {
                tr {
                    th { "Title" }
                    th { "Category" }
                    th { "Unread" }
                    @if page.open_rate_shown {
                        th { "Open Rate" }
                    }
                    th { "Actions" }
                }
            }
            tbody data-testid="feeds-table" {
                @for feed in &page.feeds {
                    (feed_row(page, feed))
                    @if let Some(err) = &feed.fetch_error {
                        tr class="error-row" data-feed-id=(feed.id) data-is-error-row="true" {
                            td colspan=(colspan) class="error-text feed-error-cell" { "Error: " (err) }
                        }
                    }
                }
                @if page.feeds.is_empty() {
                    tr { td colspan=(colspan) class="empty-state-compact" { "No feeds yet — add one using the form above." } }
                }
            }
        }
    }
}

fn feed_row(page: &FeedsTemplate, feed: &FeedRowView) -> Markup {
    let has_error = feed.fetch_error.is_some();
    let error_class = has_error.then_some(ERROR_ROW_CLASS);
    html! {
        tr id={ "row-feed-" (feed.id) } data-feed-id=(feed.id) class=[error_class] {
            td data-label="Title" class=[error_class] {
                div class="feed-title-cell" {
                    div class="feed-title-line" {
                        @if feed.has_icon {
                            img src={ "/api/feeds/" (feed.id) "/icon" } alt="" class="feed-icon" data-hide-on-error;
                            " "
                        }
                        span title=(feed.url) { (feed.title) }
                    }
                    div class="feed-health-info" {
                        span class="muted" { "Fetched: " (relative_time(&feed.fetched_at_datetime, &feed.fetched_at_relative)) }
                        " · "
                        span class=(feed.freshness_class) { "Updated: " (relative_time(&feed.feed_updated_at_datetime, &feed.feed_updated_at_relative)) }
                    }
                }
            }
            td data-label="Category" class=[error_class] { (feed.category_name) }
            td data-label="Unread" class=[error_class] {
                @if feed.unread_count > 0 {
                    strong { (feed.unread_count) }
                } @else {
                    "0"
                }
            }
            @if page.open_rate_shown {
                td data-label="Open Rate" class=[error_class] data-testid="feed-open-rate" {
                    @if let Some(label) = &feed.open_rate_label {
                        (label)
                    } @else {
                        span class="muted" title={ "Needs at least " (page.min_tracked_for_rate) " tracked entries" } { "—" }
                    }
                }
            }
            td class={ "actions" @if has_error { " " (ERROR_ROW_CLASS) } } {
                (feed_actions(page, feed))
            }
        }
    }
}

/// `<time>` whose `title` repeats the timestamp only when there is one.
fn relative_time(datetime: &str, relative: &str) -> Markup {
    let title = (!datetime.is_empty()).then_some(datetime);
    html! {
        time datetime=(datetime) title=[title] { (relative) }
    }
}

fn feed_actions(page: &FeedsTemplate, feed: &FeedRowView) -> Markup {
    html! {
        a href={ "/feeds/" (feed.id) "/entries" } { "entries" }
        " "
        form method="post" action={ "/feeds/" (feed.id) "/refresh" } class="form-inline" {
            (macros::csrf_field(&page.csrf_token))
            " "
            input type="hidden" name="return_to" value=(page.return_to);
            " "
            button type="submit" class="action-link" { "refresh" }
        }
        " "
        a href={ "/feeds/" (feed.id) "/edit?" (page.edit_query) } { "edit" }
        " "
        form method="post" action={ "/feeds/" (feed.id) "/delete" } class="form-inline" data-confirm="Delete this feed? This cannot be undone." {
            (macros::csrf_field(&page.csrf_token))
            " "
            input type="hidden" name="return_to" value=(page.return_to);
            " "
            button type="submit" class="action-link action-link-danger" { "delete" }
        }
    }
}

pub(crate) fn edit_page(page: &FeedEditTemplate) -> Markup {
    let layout = &page.layout;
    let body = html! {
        div class="app-layout" {
            rdrs-sidebar active="feeds" {}
            main class="main-content" {
                (macros::flash_mount(&layout.flash_messages))
                div class="page-content" {
                    h1 { "Edit Feed" }
                    (edit_form(page))
                    hr;
                    (refetch_metadata_form(page))
                }
            }
        }
    };
    layout::app_page(layout, page.title, None, body)
}

fn edit_form(page: &FeedEditTemplate) -> Markup {
    let feed = &page.feed;
    html! {
        form method="post" action={ "/feeds/" (feed.id) "/edit" } {
            (macros::csrf_field(&page.csrf_token))
            " "
            input type="hidden" name="return_to" value=(page.return_to);
            " "
            div class="form-group" {
                label for="url" { "Feed URL" }
                " "
                input type="text" id="url" name="url" value=(feed.url) required data-testid="feed-edit-url-input";
            }
            div class="form-group" {
                label for="title" { "Title" }
                " "
                input type="text" id="title" name="title" value=(feed.title) data-testid="feed-edit-title-input";
            }
            div class="form-group" {
                label for="description" { "Description" }
                " "
                input type="text" id="description" name="description" value=(feed.description);
            }
            div class="form-group" {
                label for="site_url" { "Site URL" }
                " "
                input type="text" id="site_url" name="site_url" value=(feed.site_url);
            }
            div class="form-group" {
                label for="category_id" { "Category" }
                " "
                select id="category_id" name="category_id" required data-testid="feed-edit-category-select" {
                    @for c in &page.categories {
                        (category_option(c, feed.category_id))
                    }
                }
            }
            (http_settings(page))
            div class="modal-actions" {
                button type="submit" data-testid="feed-edit-save-btn" { "Save" }
                " "
                a href=(page.return_to) class="btn btn-secondary" { "Cancel" }
            }
        }
    }
}

fn category_option(category: &FeedCategoryOption, selected_id: i64) -> Markup {
    html! {
        option value=(category.id) selected[category.id == selected_id] { (category.name) }
    }
}

fn http_settings(page: &FeedEditTemplate) -> Markup {
    let feed = &page.feed;
    let has_referrer_suggestions = !feed.referrer_suggestions.is_empty();
    let referrer_list = has_referrer_suggestions.then_some("referrer-list");
    html! {
        details class="feed-http-settings" {
            summary { "HTTP Settings" }
            div class="feed-http-settings-body" {
                div class="form-group" {
                    label for="custom_user_agent" { "Custom User Agent" }
                    " "
                    input type="text" id="custom_user_agent" name="custom_user_agent" value=(feed.custom_user_agent) list="user-agent-list" autocomplete="off";
                    datalist id="user-agent-list" {
                        @for ua in page.user_agent_suggestions {
                            option value=(ua);
                        }
                    }
                    div class="feed-http-hint" { "Leave empty to use the server's own user agent" }
                }
                div class="form-group" {
                    label for="custom_referrer" { "Custom Referrer" }
                    " "
                    input type="text" id="custom_referrer" name="custom_referrer" value=(feed.custom_referrer) list=[referrer_list] autocomplete="off";
                    @if has_referrer_suggestions {
                        datalist id="referrer-list" {
                            @for r in &feed.referrer_suggestions {
                                option value=(r);
                            }
                        }
                    }
                    div class="feed-http-hint" { "Some image servers require a specific Referer header to serve images" }
                }
                div class="form-group" {
                    label {
                        " "
                        input type="checkbox" id="http2_disabled" name="http2_disabled" checked[feed.http2_disabled];
                        " Disable HTTP/2 "
                    }
                    div class="feed-http-hint" { "Enable this if the feed server has HTTP/2 compatibility issues" }
                }
            }
        }
    }
}

fn refetch_metadata_form(page: &FeedEditTemplate) -> Markup {
    html! {
        form method="post" action={ "/feeds/" (page.feed.id) "/fetch-metadata" } data-confirm="Re-fetch metadata from the feed URL? This will overwrite current title/description/site URL." {
            (macros::csrf_field(&page.csrf_token))
            " "
            input type="hidden" name="return_to" value=(page.return_to);
            " "
            button type="submit" class="btn-secondary" { "Re-fetch Metadata" }
        }
    }
}

pub(crate) fn import_page(page: &FeedsImportTemplate) -> Markup {
    let layout = &page.layout;
    let body = html! {
        div class="app-layout" {
            rdrs-sidebar active="feeds" {}
            main class="main-content" {
                (macros::flash_mount(&layout.flash_messages))
                div class="page-content" {
                    h1 { "Import OPML" }
                    form method="post" action="/feeds/import" enctype="multipart/form-data" {
                        // Multipart bodies bypass `csrf_guard`; the handler validates this itself.
                        (macros::csrf_field(&page.csrf_token))
                        " "
                        div class="form-group" {
                            label for="file" { "Upload .opml file" }
                            " "
                            input type="file" id="file" name="file" accept=".opml,.xml" data-testid="opml-file-input";
                        }
                        div class="form-group" {
                            label for="content" { "Or paste OPML content" }
                            " "
                            textarea id="content" name="content" rows="10" class="textarea-full" {}
                        }
                        div class="modal-actions" {
                            button type="submit" data-testid="opml-import-btn" { "Import" }
                            " "
                            a href="/feeds" class="btn btn-secondary" { "Cancel" }
                        }
                    }
                }
            }
        }
    };
    layout::app_page(layout, page.title, None, body)
}
