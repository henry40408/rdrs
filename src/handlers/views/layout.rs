//! Document shell and logged-in app shell.

use maud::{DOCTYPE, Markup, PreEscaped, html};

use super::{icons, macros};
use crate::handlers::pages::AppLayoutContext;

/// Attributes only logged-in pages put on `<html>`. `offline.js` reads them
/// before its first network call, to wipe another account's cache.
pub(crate) struct AppRoot<'a> {
    pub theme: Option<&'a str>,
    pub offline_key: &'a str,
    pub offline_keep: i64,
}

/// Inputs of [`document`]; each field fills one part of the document.
pub(crate) struct DocumentParts<'a> {
    /// `{% block title %}`: the complete `<title>` text, e.g. "Login - RDRS".
    pub title: &'a str,
    /// `{{ git_version }}`, stamped on asset URLs.
    pub git_version: &'a str,
    /// `{% block html_attrs %}`: `Some` for logged-in pages, `None` otherwise.
    pub root: Option<AppRoot<'a>>,
    /// `{% block head %}`: extra `<head>` children after the common ones.
    pub head: Markup,
    /// `{% block body %}` (and the `flash` / `content` blocks nested in it,
    /// which no template overrides). The icon sprite is added automatically.
    pub body: Markup,
}

/// The whole HTML document, for pages that bring their own shell
/// (login, setup, invite, offline).
pub(crate) fn document(parts: DocumentParts<'_>) -> Markup {
    let v = parts.git_version;
    let root = parts.root.as_ref();
    html! {
        (DOCTYPE)
        html lang="en" data-theme=[root.and_then(|r| r.theme)] data-offline-key=[root.map(|r| r.offline_key)] data-offline-keep=[root.map(|r| r.offline_keep)] {
            head {
                meta charset="UTF-8";
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                title { (parts.title) }
                // Version-stamped so a cached icon cannot outlive an upgrade.
                link rel="icon" href={ "/favicon.ico?v=" (v) } sizes="32x32";
                link rel="icon" href={ "/favicon.svg?v=" (v) } type="image/svg+xml";
                link rel="apple-touch-icon" href={ "/apple-touch-icon.png?v=" (v) };
                // `crossorigin` makes the manifest request carry cookies; without it a
                // cookie-authenticating reverse proxy redirects it and installability breaks.
                link rel="manifest" href={ "/static/manifest.webmanifest?v=" (v) } crossorigin="use-credentials";
                // Two entries so browser chrome follows the system colour scheme (a
                // `<meta>` cannot follow the `data-theme` override). Values are `--color-paper`.
                meta name="theme-color" media="(prefers-color-scheme: light)" content="#F2F1ED";
                meta name="theme-color" media="(prefers-color-scheme: dark)" content="#131416";
                link rel="stylesheet" href={ "/static/css/app.css?v=" (v) };
                // Loaded first so its fetch patch attaches the CSRF token before any
                // state-changing request.
                (module_script(v, "csrf.js"))
                (modulepreload(v, "components/rdrs-flash.js"))
                (module_script(v, "components/rdrs-flash.js"))
                (parts.head)
            }
            body {
                // Outside the body block so every page gets it; swap fragments reference its ids.
                (icons::sprite())
                (parts.body)
            }
        }
    }
}

/// `<script type="module" src="/static/js/{path}?v={git_version}">`, for
/// `head` or `page_script`.
pub(crate) fn module_script(git_version: &str, path: &str) -> Markup {
    html! { script type="module" src={ "/static/js/" (path) "?v=" (git_version) } {} }
}

fn modulepreload(git_version: &str, path: &str) -> Markup {
    html! { link rel="modulepreload" href={ "/static/js/" (path) "?v=" (git_version) }; }
}

/// `<head>` additions for logged-in pages, then the page's own script.
fn app_head(layout: &AppLayoutContext, page_script: Option<Markup>) -> Markup {
    let v = layout.git_version;
    html! {
        @for path in ["components/rdrs-kb-help.js", "components/rdrs-sidebar.js", "keyboard.js", "bulk-read.js", "entries-search.js", "live-updates.js", "summary.js", "app.js"] {
            (modulepreload(v, path))
        }
        (module_script(v, "components/rdrs-kb-help.js"))
        (module_script(v, "components/rdrs-sidebar.js"))
        (module_script(v, "app.js"))
        // Delegated handlers replacing inline `on*` handlers (strict CSP).
        (module_script(v, "behaviors.js"))
        // Not in the base document so /login and /setup never register a worker.
        // No modulepreload: it must not compete with the modules that paint.
        (module_script(v, "pwa.js"))
        // Offline reading; runs after paint, no modulepreload.
        (module_script(v, "offline.js"))
        // Reports browser-only facts once per change; after paint, no modulepreload.
        (module_script(v, "client-info.js"))
        @if let Some(script) = page_script { (script) }
    }
}

/// No-JS navigation, since `<rdrs-sidebar>` renders nothing without script.
/// Destinations only, to keep bytes low; inside `<noscript>` it is not parsed
/// when scripting is on, so `:has(.nav-fallback)` never matches. Whitespace
/// between the items is omitted: the nav is a flex container.
fn nav_fallback(layout: &AppLayoutContext) -> Markup {
    html! {
        noscript {
            nav.nav-fallback aria-label="Main navigation" {
                a.nav-fallback-brand href="/" { "rdrs" }
                a href="/" {
                    "Unread"
                    @if layout.total_unread > 0 { " " span.nav-fallback-count { (layout.total_unread) } }
                }
                a href="/entries/today" { "Today" }
                a href="/entries" { "All" }
                a href="/entries/starred" { "Starred" }
                a href="/entries/summarized" { "Summarized" }
                // Only for readers with offline reading on.
                @if layout.offline_keep > 0 { a href="/entries/offline" { "Offline" } }
                a href="/categories" { "Categories" }
                a href="/feeds" { "Feeds" }
                a href="/search" { "Search" }
                a href="/summarizer" { "Summarizer" }
                a href="/statistics" { "Statistics" }
                a href="/user-settings" { "Preferences" }
                a href="/settings" { "Settings" }
                @if layout.is_admin { a href="/admin" { "Admin" } }
                // The scripted sign-out is a fetch DELETE; a form needs `POST /logout`.
                form.nav-fallback-logout method="post" action="/logout" {
                    (macros::csrf_field(&layout.csrf_token))
                    " "
                    button type="submit" data-testid="nav-fallback-logout" { "Sign out" }
                }
            }
        }
    }
}

/// A logged-in page, with the sidebar, flash banners and navigation.
///
/// `title` is rendered as "{title} - RDRS". `page_script` is an optional
/// module script (build it with [`module_script`]). Each page calls
/// [`macros::flash_mount`] inside `page`.
///
/// `layout.sidebar_bootstrap_json` is the one `PreEscaped`: it comes from
/// `serialize_sidebar_for_script`, which escapes `</` to `<\/`, so it cannot
/// close the `<script>` element.
pub(crate) fn app_page(
    layout: &AppLayoutContext,
    title: &str,
    page_script: Option<Markup>,
    page: Markup,
) -> Markup {
    let title = format!("{title} - RDRS");
    let body = html! {
        script type="application/json" id="rdrs-sidebar-bootstrap" { (PreEscaped(&layout.sidebar_bootstrap_json)) }
        (nav_fallback(layout))
        (page)
        rdrs-kb-help data-testid="kb-help" {}
    };
    document(DocumentParts {
        title: &title,
        git_version: layout.git_version,
        root: Some(AppRoot {
            theme: layout.theme.as_deref(),
            offline_key: &layout.offline_key,
            offline_keep: layout.offline_keep,
        }),
        head: app_head(layout, page_script),
        body,
    })
}
