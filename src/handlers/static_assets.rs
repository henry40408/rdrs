use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

// Embed all static assets at compile time for single-binary deployment
const FILES: &[(&str, &str)] = &[
    ("css/app.css", include_str!("../../static/css/app.css")),
    ("js/app.js", include_str!("../../static/js/app.js")),
    ("js/passkey.js", include_str!("../../static/js/passkey.js")),
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
    (
        "js/components/rdrs-reading-chart.js",
        include_str!("../../static/js/components/rdrs-reading-chart.js"),
    ),
];

fn content_type_for(path: &str) -> &'static str {
    if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else {
        "application/javascript"
    }
}

pub async fn serve(Path(path): Path<String>) -> Response {
    match FILES.iter().find(|(name, _)| *name == path) {
        Some((name, content)) => {
            // `?v=…-dirty` URLs are emitted from a working tree with
            // uncommitted changes; the suffix is identical across consecutive
            // dev edits, so browsers would serve a stale cached copy under
            // the long-lived immutable header. Switch to `no-cache` for that
            // case so dev iteration sees fresh assets without a hard refresh.
            let cache_control = if crate::GIT_VERSION.ends_with("-dirty") {
                "no-cache"
            } else {
                "public, max-age=31536000, immutable"
            };
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type_for(name)),
                    (header::CACHE_CONTROL, cache_control),
                ],
                *content,
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
