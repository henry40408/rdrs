use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

// Embed all static assets at compile time for single-binary deployment
const FILES: &[(&str, &str)] = &[
    ("css/app.css", include_str!("../../static/css/app.css")),
    ("js/utils.js", include_str!("../../static/js/utils.js")),
    (
        "js/keyboard.js",
        include_str!("../../static/js/keyboard.js"),
    ),
    (
        "js/components/rdrs-entry-list.js",
        include_str!("../../static/js/components/rdrs-entry-list.js"),
    ),
    (
        "js/components/rdrs-flash.js",
        include_str!("../../static/js/components/rdrs-flash.js"),
    ),
    (
        "js/components/rdrs-kb-help.js",
        include_str!("../../static/js/components/rdrs-kb-help.js"),
    ),
    (
        "js/components/rdrs-kb-pending.js",
        include_str!("../../static/js/components/rdrs-kb-pending.js"),
    ),
    (
        "js/components/rdrs-sidebar.js",
        include_str!("../../static/js/components/rdrs-sidebar.js"),
    ),
    (
        "js/pages/statistics.js",
        include_str!("../../static/js/pages/statistics.js"),
    ),
    (
        "js/pages/categories.js",
        include_str!("../../static/js/pages/categories.js"),
    ),
    (
        "js/pages/feeds.js",
        include_str!("../../static/js/pages/feeds.js"),
    ),
    (
        "js/pages/settings.js",
        include_str!("../../static/js/pages/settings.js"),
    ),
    (
        "js/pages/user-settings.js",
        include_str!("../../static/js/pages/user-settings.js"),
    ),
    (
        "js/pages/admin.js",
        include_str!("../../static/js/pages/admin.js"),
    ),
    (
        "js/pages/entries.js",
        include_str!("../../static/js/pages/entries.js"),
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
        Some((name, content)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type_for(name)),
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            *content,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
