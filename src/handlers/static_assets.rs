use axum::{
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

// Embed all static assets at compile time for single-binary deployment
const FILES: &[(&str, &str)] = &[
    ("css/app.css", include_str!("../../static/css/app.css")),
    ("js/app.js", include_str!("../../static/js/app.js")),
    ("js/passkey.js", include_str!("../../static/js/passkey.js")),
    (
        "js/statistics.js",
        include_str!("../../static/js/statistics.js"),
    ),
    ("js/utils.js", include_str!("../../static/js/utils.js")),
    (
        "js/components/rdrs-flash.js",
        include_str!("../../static/js/components/rdrs-flash.js"),
    ),
    (
        "js/components/rdrs-kb-help.js",
        include_str!("../../static/js/components/rdrs-kb-help.js"),
    ),
    (
        "js/components/rdrs-sidebar.js",
        include_str!("../../static/js/components/rdrs-sidebar.js"),
    ),
];

// Self-hosted webfonts (latin subset). Binary, so embedded via `include_bytes!`
// and served ahead of the text `FILES` table. Vendored to avoid a Google Fonts
// CDN dependency (privacy for self-hosted deployments); SIL OFL licensed, see
// static/fonts/OFL-*.txt.
const FONTS: &[(&str, &[u8])] = &[
    (
        "fonts/newsreader-latin.woff2",
        include_bytes!("../../static/fonts/newsreader-latin.woff2"),
    ),
    (
        "fonts/newsreader-italic-latin.woff2",
        include_bytes!("../../static/fonts/newsreader-italic-latin.woff2"),
    ),
    (
        "fonts/archivo-latin.woff2",
        include_bytes!("../../static/fonts/archivo-latin.woff2"),
    ),
    (
        "fonts/ibm-plex-mono-400-latin.woff2",
        include_bytes!("../../static/fonts/ibm-plex-mono-400-latin.woff2"),
    ),
    (
        "fonts/ibm-plex-mono-500-latin.woff2",
        include_bytes!("../../static/fonts/ibm-plex-mono-500-latin.woff2"),
    ),
    (
        "fonts/ibm-plex-mono-600-latin.woff2",
        include_bytes!("../../static/fonts/ibm-plex-mono-600-latin.woff2"),
    ),
];

fn content_type_for(path: &str) -> &'static str {
    if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else {
        "application/javascript"
    }
}

fn cache_control_for() -> &'static str {
    // `?v=…-dirty` URLs are emitted from a working tree with uncommitted
    // changes; the suffix is identical across consecutive dev edits, so
    // browsers would serve a stale cached copy under the long-lived immutable
    // header. Switch to `no-cache` for that case so dev iteration sees fresh
    // assets without a hard refresh.
    if crate::GIT_VERSION.ends_with("-dirty") {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    }
}

pub async fn serve(Path(path): Path<String>) -> Response {
    if let Some((name, bytes)) = FONTS.iter().find(|(name, _)| *name == path) {
        return (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type_for(name)),
                (header::CACHE_CONTROL, cache_control_for()),
            ],
            *bytes,
        )
            .into_response();
    }
    match FILES.iter().find(|(name, _)| *name == path) {
        Some((name, content)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type_for(name)),
                (header::CACHE_CONTROL, cache_control_for()),
            ],
            *content,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
