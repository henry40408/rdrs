//! The `/summarizer` page; the per-card fragment is `macros::summarizer_card`.

use maud::{Markup, html};

use super::{layout, macros};
use crate::handlers::summarizer::SummarizerTemplate;

pub(crate) fn page(template: &SummarizerTemplate) -> Markup {
    let script = layout::module_script(template.layout.git_version, "summarizer.js");
    let body = html! {
        div.app-layout {
            rdrs-sidebar active="summarizer" {}
            main.main-content {
                (macros::flash_mount(&template.layout.flash_messages))
                div.page-content.page-content-full {
                    h1 { "Summarizer" }
                    p.page-lead { "Paste one or more links and get a Kagi summary for each, one after another — without adding them to a feed. Nothing here is saved to your library." }
                    @if template.kagi_configured { (form_and_results(template)) } @else { (not_configured()) }
                }
            }
        }
    };
    layout::app_page(&template.layout, template.title, Some(script), body)
}

fn not_configured() -> Markup {
    html! {
        div.empty-state {
            p.empty-state-text {
                "Kagi isn’t configured yet. Add your Kagi session link in "
                a href="/user-settings" { "Settings" }
                " to use the summarizer."
            }
        }
    }
}

fn form_and_results(template: &SummarizerTemplate) -> Markup {
    html! {
        form method="post" action="/summarizer" data-testid="summarizer-form" data-summarizer-form {
            (macros::csrf_field(&template.csrf_token))
            " "
            label.field-label for="sz-urls" { "Links to summarize" }
            // A literal newline in the attribute, equivalent to `&#10;`.
            textarea #sz-urls name="urls" rows="6" spellcheck="false" class="textarea-full" placeholder="https://example.com/article\nhttps://another.com/post" data-testid="summarizer-input" { (template.urls_text) }
            @if let Some(err) = &template.error { p.form-error data-testid="summarizer-error" { (err) } }
            div.form-actions {
                span.muted { "One URL per line · up to 30" }
                " "
                button.btn type="submit" { "Summarize" }
            }
        }
        div #sz-results data-summarizer-results {
            @for card in &template.cards { (macros::summarizer_card(card)) }
        }
    }
}
