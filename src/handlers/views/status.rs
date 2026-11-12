//! Offline and error pages.

use maud::{Markup, html};

use super::layout::{DocumentParts, app_page, document};
use super::macros;
use crate::handlers::pages::{ErrorTemplate, OfflineTemplate};

/// Served from the SW cache to everyone: no user data, and a plain link rather
/// than a scripted retry, since scripts may be what failed. The saved-entries
/// link is unconditional because this shared page cannot know if a library exists.
pub(crate) fn offline(page: &OfflineTemplate) -> Markup {
    let body = html! {
        div.auth-layout {
            div.auth-card data-testid="offline-page" {
                h1 { "rdrs" }
                p.offline-message { "You are offline." }
                p.offline-message.muted { "Feeds live on the server, so syncing, searching and marking entries have to wait for the connection. Anything you saved for offline reading is still here." }
                div.link {
                    a href="/entries/offline" data-testid="offline-library-link" { "Saved entries" }
                    " "
                    a href="/" { "Try again" }
                }
            }
        }
    };
    document(DocumentParts {
        title: "Offline - RDRS",
        git_version: page.git_version,
        root: None,
        head: html! {},
        body,
    })
}

pub(crate) fn error(page: &ErrorTemplate) -> Markup {
    let layout = &page.layout;
    let body = html! {
        div.app-layout {
            rdrs-sidebar {}
            main.main-content {
                (macros::flash_mount(&layout.flash_messages))
                div.page-content {
                    h1 { (page.heading) }
                    p.muted { (page.message) }
                    p { a href="/" { "Back to Unread" } }
                }
            }
        }
    };
    app_page(layout, page.title, None, body)
}
