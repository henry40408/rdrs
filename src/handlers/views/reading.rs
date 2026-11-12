//! Reading pane, summary container and the entry-action swap responses.

use maud::{Markup, PreEscaped, html};

use super::{icons, macros};
use crate::handlers::entries::{
    EntryActionMulti, FlashPayload, OpenEntryMulti, ReadingPaneFragment, ReadingPaneWithFlash,
    SummarizePending, SummaryFragment,
};
use crate::handlers::pages::ReadingPaneView;

/// Empty reading pane; the category swap uses it so the old entry never
/// lingers. It must not carry `reading-pane-active`, or mobile users get
/// trapped behind a blank pane.
pub(crate) fn empty_pane() -> Markup {
    html! {
        div #reading-pane.reading-pane.reading-pane-empty {
            p.empty-state-compact { "Select an entry from the list to start reading." }
        }
    }
}

pub(crate) fn pane_fragment(fragment: &ReadingPaneFragment) -> Markup {
    pane(&fragment.pane, &fragment.csrf_token)
}

/// Reading pane plus an optional toast for Save / Fetch Full Content. The toast
/// is the only feedback, since these actions change no row state.
pub(crate) fn pane_with_flash(fragment: &ReadingPaneWithFlash) -> Markup {
    html! {
        template data-swap-target="#reading-pane" { (pane(&fragment.pane, &fragment.csrf_token)) }
        @if let Some(flash) = &fragment.flash { (flash_template(flash)) }
    }
}

fn flash_template(flash: &FlashPayload) -> Markup {
    html! { template data-flash data-level=(flash.level) { (flash.message) } }
}

/// `POST /entries/{id}/summarize`: swaps only the summary container so a
/// Fetch-Full-Content view stays put.
pub(crate) fn summarize_pending(fragment: &SummarizePending) -> Markup {
    html! {
        template data-swap-target="#rp-summary-container" {
            (summary_container(pending_box(fragment.id, &fragment.csrf_token)))
        }
    }
}

/// `POST /entries/{id}/summarize/cancel`: resets the container to empty.
pub(crate) fn summary_cleared() -> Markup {
    html! {
        template data-swap-target="#rp-summary-container" { (summary_container(html! {})) }
    }
}

/// `GET /entries/{id}/summary/fragment`: lets an SSE summary event swap
/// `#rp-summary-container`.
pub(crate) fn summary_fragment(fragment: &SummaryFragment) -> Markup {
    html! {
        template data-swap-target="#rp-summary-container" {
            (summary_container(summary_inner(&fragment.pane, &fragment.csrf_token)))
        }
    }
}

/// Row actions (read / unread / star / unstar): re-renders only the two
/// stateful forms plus an `entry-read` class directive. Both forms always ship
/// so the response is action-agnostic; selectors missing from the page are
/// skipped.
pub(crate) fn entry_actions_multi(response: &EntryActionMulti) -> Markup {
    let row = &response.row;
    let csrf = response.csrf_token.as_str();
    html! {
        template data-swap-target={ "#entry-row-" (row.id) " .entry-marker" } { (macros::row_marker_form(row.id, row.is_read, csrf)) }
        template data-swap-target={ "#entry-row-" (row.id) " .entry-star-form" } { (macros::row_star_form(row.id, row.is_starred, csrf)) }
        (read_class_directive(row.id, row.is_read))
        @if let Some(flash) = &response.flash { (flash_template(flash)) }
        @if let Some(star) = &response.pane_star_form {
            template data-swap-target={ "#reading-pane-star-form-" (star.id) } { (macros::pane_star_form(star.id, star.is_starred, csrf, false)) }
            template data-swap-target={ "#reading-pane-bar-star-form-" (star.id) } { (macros::pane_star_form(star.id, star.is_starred, csrf, true)) }
        }
    }
}

/// `GET /entries/{id}/fragment`: the templates override the link's data-swap to
/// update the reading pane and mark the row read in one round trip. Sidebar
/// counts arrive over SSE; the star form is absent because opening never
/// touches it.
pub(crate) fn open_entry_multi(response: &OpenEntryMulti) -> Markup {
    let row = &response.row;
    html! {
        template data-swap-target="#reading-pane" { (pane(&response.pane, &response.csrf_token)) }
        template data-swap-target={ "#entry-row-" (row.id) " .entry-marker" } { (macros::row_marker_form(row.id, row.is_read, &response.csrf_token)) }
        (read_class_directive(row.id, row.is_read))
    }
}

fn read_class_directive(row_id: i64, is_read: bool) -> Markup {
    html! {
        @if is_read {
            template data-class-target={ "#entry-row-" (row_id) } data-class-add="entry-read" {}
        } @else {
            template data-class-target={ "#entry-row-" (row_id) } data-class-remove="entry-read" {}
        }
    }
}

/// `reading-pane-active` reveals the pane as a full-screen overlay on mobile
/// (<=1024px); empty states must not carry it.
pub(crate) fn pane(pane: &ReadingPaneView, csrf: &str) -> Markup {
    html! {
        div #reading-pane.reading-pane.reading-pane-active data-entry-id=(pane.id) {
            (toolbar(pane, csrf))
            div.reading-pane-content {
                (eyebrow(pane))
                (headline(pane))
                @if let Some(author) = &pane.author {
                    div.byline { span.author { (author) } }
                }
                (action_row(pane, csrf))
                // Always rendered so a summary swap never resets a Fetch-Full-Content view.
                (summary_container(summary_inner(pane, csrf)))
                // `lang` on headline and body; see `utils::han`. `content_html` is
                // sanitized upstream (`sanitize_html`), hence PreEscaped.
                article.reading-pane-article data-testid="reading-pane-body" data-mode=(if pane.is_full_content { "full" } else { "original" }) lang=[pane.content_lang()] {
                    (PreEscaped(&pane.content_html))
                }
            }
        }
    }
}

fn favicon(pane: &ReadingPaneView, aria_hidden_chip: bool) -> Markup {
    html! {
        @if pane.feed_has_icon {
            img.entry-favicon src={ "/api/feeds/" (pane.feed_id) "/icon" } alt="" decoding="sync" width="15" height="15";
        } @else {
            span class={ "entry-favicon entry-favicon-chip fav-c" (pane.feed_color_index()) } aria-hidden=[aria_hidden_chip.then_some("true")] { (pane.feed_initial()) }
        }
    }
}

/// Toolbar: mobile Back on the left, prev/next on the right. Desktop shows
/// Star/Unread on the left (the <=1024px action bar carries them instead) and,
/// once the headline scrolls away, its title; the bottom edge is a reading
/// progress line (`--rp-progress`, set by app.js).
fn toolbar(pane: &ReadingPaneView, csrf: &str) -> Markup {
    html! {
        div.reading-pane-back {
            div.reading-pane-bar-actions {
                (macros::pane_star_form(pane.id, pane.is_starred, csrf, true))
                (macros::pane_unread_form(pane.id, csrf, true))
            }
            span.reading-pane-bar-title aria-hidden="true" {
                " "
                (favicon(pane, false))
                " "
                span.reading-pane-bar-title-text lang=[pane.title_lang()] { (pane.title) }
                " "
            }
            " "
            button.reading-pane-back-link type="button" data-pane-back data-testid="reading-pane-back" {
                span.action-icon aria-hidden="true" { (icons::chevron_left()) }
                span { "Back" }
            }
            " "
            // "12 / 48", filled in by app.js from `/api/entries/{id}/position`.
            span.reading-pane-bar-pos data-pane-pos data-testid="reading-pane-position" hidden {}
            " "
            // Rendered disabled; app.js enables them via `/api/entries/{id}/neighbors`.
            // "Previous" = newer, "Next" = older (published-desc order).
            nav.reading-pane-nav aria-label="Entry navigation" {
                button.reading-pane-nav-btn type="button" data-pane-prev data-testid="reading-pane-prev" aria-label="Previous" disabled {
                    (icons::chevron_left())
                    span.nav-label { "Previous" }
                }
                " "
                button.reading-pane-nav-btn type="button" data-pane-next data-testid="reading-pane-next" aria-label="Next" disabled {
                    span.nav-label { "Next" }
                    (icons::chevron_right())
                }
            }
        }
    }
}

/// Eyebrow: favicon + feed name, then published time.
fn eyebrow(pane: &ReadingPaneView) -> Markup {
    html! {
        div.dispatch-eyebrow {
            span.src {
                " "
                (favicon(pane, true))
                " "
                span data-testid="reading-pane-feed-title" { (pane.feed_title) }
                " "
            }
            @if let Some(timestamp) = &pane.published_at_iso {
                " "
                time datetime=(timestamp) data-testid="reading-pane-published-at" { (pane.published_relative) }
            }
        }
    }
}

fn headline(pane: &ReadingPaneView) -> Markup {
    html! {
        h1.reading-pane-title data-testid="reading-pane-title" lang=[pane.title_lang()] {
            @if let Some(link) = &pane.link {
                a href=(link) target="_blank" rel="noopener noreferrer" { (pane.title) }
            } @else {
                (pane.title)
            }
        }
    }
}

fn action_row(pane: &ReadingPaneView, csrf: &str) -> Markup {
    html! {
        div.reading-pane-actions {
            // Above 1024px these two live in the toolbar instead.
            (macros::pane_star_form(pane.id, pane.is_starred, csrf, false))
            (macros::pane_unread_form(pane.id, csrf, false))
            @if pane.link.is_some() {
                (full_content_action(pane, csrf))
                @if pane.has_kagi || pane.summary_in_flight { (summary_toggle(pane, csrf)) }
                @if pane.has_save { (save_form(pane.id, csrf)) }
            }
        }
    }
}

fn full_content_action(pane: &ReadingPaneView, csrf: &str) -> Markup {
    let id = pane.id;
    html! {
        @if pane.is_full_content {
            // `?view=original` is needed, else the stored fetched article comes back.
            a.rp-action.rp-slot-wide.is-on href={ "/entries/" (id) "/fragment?view=original" } data-swap="#reading-pane" aria-label="Show Original" {
                span.action-icon aria-hidden="true" { (icons::revert()) }
                span.action-label { "Original" }
                kbd { "d" }
            }
        } @else if pane.has_stored_full_content {
            // A plain link back, so undoing the toggle never re-fetches the origin server.
            a.rp-action.rp-slot-wide href={ "/entries/" (id) "/fragment" } data-swap="#reading-pane" aria-label="Show Full Content" {
                span.action-icon aria-hidden="true" { (icons::document()) }
                span.action-label { "Full text" }
                kbd { "d" }
            }
        } @else {
            form.rp-slot-wide method="post" action={ "/entries/" (id) "/fetch-full-content" } data-swap="#reading-pane" data-cancel-swap {
                (macros::csrf_field(csrf))
                " "
                button.rp-action type="submit" aria-label="Fetch Full Content" data-cancel-label="Cancel" data-cancel-aria-label="Cancel" {
                    span.action-icon.action-icon-default aria-hidden="true" { (icons::document()) }
                    span.action-icon.action-icon-cancel aria-hidden="true" hidden { (icons::close()) }
                    span.action-label { "Full text" }
                    kbd { "d" }
                }
            }
        }
    }
}

fn summary_toggle(pane: &ReadingPaneView, csrf: &str) -> Markup {
    let has_summary = pane.summary_text.is_some();
    html! {
        form.rp-slot-wide method="post" action={ "/entries/" (pane.id) "/summarize" } data-swap="#rp-summary-container" data-summary-toggle {
            (macros::csrf_field(csrf))
            " "
            button.rp-action type="submit" aria-label=(if has_summary { "Dismiss summary" } else { "Summarize" }) disabled[pane.summary_in_flight] {
                span.action-icon.action-icon-summarize aria-hidden="true" hidden[has_summary] { (icons::sparkle()) }
                span.action-icon.action-icon-dismiss aria-hidden="true" hidden[!has_summary] { (icons::close()) }
                span.action-label { (if has_summary { "Dismiss" } else { "Summary" }) }
                kbd { "a" }
            }
        }
    }
}

fn save_form(id: i64, csrf: &str) -> Markup {
    html! {
        form method="post" action={ "/entries/" (id) "/save" } data-swap="#reading-pane" {
            (macros::csrf_field(csrf))
            " "
            button.rp-action type="submit" aria-label="Save" {
                span.action-icon aria-hidden="true" { (icons::bookmark()) }
                span.action-label { "Save" }
                kbd { "s" }
            }
        }
    }
}

fn summary_container(inner: Markup) -> Markup {
    html! {
        div.rp-summary-container #rp-summary-container data-summary-container { (inner) }
    }
}

fn cancel_form(id: i64, csrf: &str) -> Markup {
    html! {
        form method="post" action={ "/entries/" (id) "/summarize/cancel" } data-swap="#rp-summary-container" {
            (macros::csrf_field(csrf))
            " "
            button.rp-action type="submit" aria-label="Cancel summarization" {
                span.action-icon aria-hidden="true" { (icons::close()) }
                span.action-label { "Cancel" }
            }
        }
    }
}

fn pending_box(id: i64, csrf: &str) -> Markup {
    html! {
        div.summary-box data-summary-pending {
            div.summary-actions { (cancel_form(id, csrf)) }
            p.muted { "Summarizing… (refresh to see the result)" }
        }
    }
}

fn summary_inner(pane: &ReadingPaneView, csrf: &str) -> Markup {
    if pane.summary_in_flight {
        return pending_box(pane.id, csrf);
    }
    if let Some(summary) = &pane.summary_text {
        return summary_box(pane, summary);
    }
    let Some(error) = &pane.summary_error else {
        return html! {};
    };
    failed_box(pane.id, error, csrf)
}

/// `summary` is `PreEscaped`: it has passed through `services::sanitize_summary`.
fn summary_box(pane: &ReadingPaneView, summary: &str) -> Markup {
    html! {
        div.summary-box {
            div.summary-actions {
                button.rp-action type="button" data-summary-copy aria-label="Copy summary" {
                    span.action-icon aria-hidden="true" { (icons::copy()) }
                    span.action-label { "Copy" }
                }
                " "
                button.rp-action type="button" data-summary-dismiss data-entry-id=(pane.id) aria-label="Dismiss summary" {
                    span.action-icon aria-hidden="true" { (icons::close()) }
                    span.action-label { "Dismiss" }
                }
            }
            div.summary-header {
                div.summary-title data-summary-title { (pane.title) }
                @if let Some(link) = &pane.link {
                    a.summary-link href=(link) target="_blank" rel="noopener noreferrer" data-summary-link { (link) }
                }
            }
            blockquote.rp-summary-content { (PreEscaped(summary)) }
        }
    }
}

fn failed_box(id: i64, error: &str, csrf: &str) -> Markup {
    html! {
        div.summary-box {
            div.summary-actions {
                form method="post" action={ "/entries/" (id) "/summarize" } data-swap="#rp-summary-container" {
                    (macros::csrf_field(csrf))
                    " "
                    button.rp-action type="submit" aria-label="Retry summarization" {
                        span.action-icon aria-hidden="true" { (icons::refresh()) }
                        span.action-label { "Retry" }
                    }
                }
                form method="post" action={ "/entries/" (id) "/summarize/cancel" } data-swap="#rp-summary-container" {
                    (macros::csrf_field(csrf))
                    " "
                    button.rp-action type="submit" aria-label="Clear failed summary" {
                        span.action-icon aria-hidden="true" { (icons::close()) }
                        span.action-label { "Clear" }
                    }
                }
            }
            div.summary-error-banner data-summary-error {
                span.action-icon aria-hidden="true" { (icons::close()) }
                span { "Summarization failed: " (error) }
            }
        }
    }
}
