//! Fragments shared by many pages.

use maud::{Markup, PreEscaped, html};
use std::fmt::Display;

use super::icons;
use crate::handlers::summarizer::SummarizerCard;
use crate::middleware::flash::FlashMessage;

pub(crate) fn current_text(value: &str) -> Markup {
    html! {
        @if value.is_empty() {
            span.muted { "Not set" }
        } @else {
            code { (value) }
        }
    }
}

/// CSRF hidden field for native POSTs, required for no-JS. `csrf.js`
/// overwrites it at submit, since a token rotation can leave this snapshot
/// stale.
pub(crate) fn csrf_field(token: &str) -> Markup {
    html! { input type="hidden" name="_csrf" value=(token); }
}

/// An entry row's read toggle. Shared with the swap responses so they cannot
/// drift. `data-swap` is only the no-template fallback.
pub(crate) fn row_marker_form(id: impl Display, is_read: bool, csrf: &str) -> Markup {
    let verb = if is_read { "Mark unread" } else { "Mark read" };
    let action = if is_read { "unread" } else { "read" };
    html! {
        form.entry-marker method="post" action={ "/entries/" (id) "/" (action) } data-swap={ "#entry-row-" (id) } {
            (csrf_field(csrf))
            button.unread-toggle type="submit" data-testid="entry-read-toggle" aria-label=(verb) title={ (verb) " (m)" } {}
        }
    }
}

/// An entry row's star toggle; see [`row_marker_form`].
pub(crate) fn row_star_form(id: impl Display, is_starred: bool, csrf: &str) -> Markup {
    let verb = if is_starred { "Unstar" } else { "Star" };
    let action = if is_starred { "unstar" } else { "star" };
    html! {
        form.entry-star-form method="post" action={ "/entries/" (id) "/" (action) } data-swap={ "#entry-row-" (id) } {
            (csrf_field(csrf))
            button.entry-star.starred[is_starred] type="submit" data-testid="entry-star-action" aria-label=(verb) title={ (verb) " (f)" } {
                (icons::star(is_starred))
            }
        }
    }
}

/// Reading-pane star button, shared with the multi-swap response. `bar`
/// picks the top-bar copy (desktop) over the action-row copy (<=1024px); both
/// ship, so their ids differ.
pub(crate) fn pane_star_form(id: impl Display, is_starred: bool, csrf: &str, bar: bool) -> Markup {
    let verb = if is_starred { "Unstar" } else { "Star" };
    let action = if is_starred { "unstar" } else { "star" };
    let form_id = if bar {
        "reading-pane-bar-star-form-"
    } else {
        "reading-pane-star-form-"
    };
    html! {
        form id={ (form_id) (id) } class=[(!bar).then_some("rp-bar-dup")] method="post" action={ "/entries/" (id) "/" (action) } data-swap={ "#entry-row-" (id) } {
            (csrf_field(csrf))
            button.rp-action.is-on[is_starred] type="submit" aria-label=(verb) {
                span.action-icon aria-hidden="true" { (icons::star(is_starred)) }
                span.action-label { (if is_starred { "Starred" } else { "Star" }) }
                kbd { "f" }
            }
        }
    }
}

/// Mark-Unread only: opening an entry already marks it read. `bar` as in
/// [`pane_star_form`].
pub(crate) fn pane_unread_form(id: impl Display, csrf: &str, bar: bool) -> Markup {
    html! {
        form class=[(!bar).then_some("rp-bar-dup")] method="post" action={ "/entries/" (id) "/unread" } data-swap={ "#entry-row-" (id) } {
            (csrf_field(csrf))
            button.rp-action type="submit" aria-label="Mark Unread" {
                span.action-icon aria-hidden="true" { (icons::unread()) }
                span.action-label { "Unread" }
                kbd { "m" }
            }
        }
    }
}

/// The banners, shared by both mounts below. Emits nothing for an empty list:
/// `.banner-stack:empty` must match, or an invisible band misaligns the
/// mobile header.
pub(crate) fn flash_banners(messages: &[FlashMessage]) -> Markup {
    html! {
        @for msg in messages {
            div class={ "banner " (msg.level_class()) } role=(msg.aria_role()) data-testid="flash-message" {
                span.banner-icon aria-hidden="true" {}
                div.banner-body {
                    span.sr-only { (msg.level_name()) ": " }
                    span.banner-message { (msg.message) }
                }
                time.banner-time datetime=(msg.timestamp_iso()) data-testid="flash-time" { (msg.formatted_time()) }
                " "
                button.banner-dismiss type="button" aria-label="Dismiss notification" data-testid="flash-close" data-flash-dismiss {
                    (icons::close())
                }
            }
        }
    }
}

/// Logged-out pages: plain container; they never flash after load.
pub(crate) fn flash(messages: &[FlashMessage]) -> Markup {
    html! {
        @if !messages.is_empty() {
            div.banner-stack role="region" aria-label="Notifications" tabindex="-1" {
                (flash_banners(messages))
            }
        }
    }
}

/// Logged-in pages: `<rdrs-flash>` is the client's toast mount, so it must
/// render even when empty. Attributes mirror what `rdrs-flash.js` sets, for
/// no-JS.
pub(crate) fn flash_mount(messages: &[FlashMessage]) -> Markup {
    html! {
        rdrs-flash.banner-stack role="region" aria-label="Notifications" tabindex="-1" {
            (flash_banners(messages))
        }
    }
}

/// One summarizer URL card. `card.summary` is `PreEscaped`: it has passed
/// through `services::sanitize_summary` (Kagi output is untrusted).
pub(crate) fn summarizer_card(card: &SummarizerCard) -> Markup {
    let completed = card.state == "completed";
    let failed = card.state == "error";
    html! {
        div.summary-box.sz-card.pending[card.state == "queued"] id={ "sz-card-" (card.index) } data-summarizer-card data-summarizer-url=(card.url) data-summarizer-index=(card.index) data-state=(card.state) {
            div.summary-actions data-sz-actions {
                @if completed {
                    button.rp-action type="button" data-sz-copy aria-label="Copy summary" { span.action-label { "Copy" } }
                    button.rp-action type="button" data-sz-dismiss aria-label="Dismiss summary" { span.action-label { "Dismiss" } }
                } @else if failed {
                    button.rp-action type="button" data-sz-retry aria-label="Retry summarization" { span.action-label { "Retry" } }
                    button.rp-action type="button" data-sz-dismiss aria-label="Dismiss" { span.action-label { "Dismiss" } }
                }
            }
            div.summary-header {
                div.summary-title data-sz-title { (card.title) }
                a.summary-link href=(card.url) target="_blank" rel="noopener noreferrer" { (card.url) }
            }
            @if completed {
                blockquote.rp-summary-content data-sz-body { (PreEscaped(&card.summary)) }
            } @else if failed {
                div.summary-error-banner data-sz-error {
                    span.action-icon aria-hidden="true" { (icons::close()) }
                    span { "Summarization failed: " (card.error) }
                }
            } @else {
                p.status data-sz-status { "Queued" }
            }
        }
    }
}
