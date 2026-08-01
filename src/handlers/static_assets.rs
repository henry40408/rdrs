use axum::{
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

// Embed all static assets at compile time for single-binary deployment
const FILES: &[(&str, &str)] = &[
    ("css/app.css", include_str!("../../static/css/app.css")),
    ("js/app.js", include_str!("../../static/js/app.js")),
    (
        "js/behaviors.js",
        include_str!("../../static/js/behaviors.js"),
    ),
    ("js/csrf.js", include_str!("../../static/js/csrf.js")),
    ("js/login.js", include_str!("../../static/js/login.js")),
    ("js/passkey.js", include_str!("../../static/js/passkey.js")),
    ("js/setup.js", include_str!("../../static/js/setup.js")),
    ("js/search.js", include_str!("../../static/js/search.js")),
    (
        "js/statistics.js",
        include_str!("../../static/js/statistics.js"),
    ),
    (
        "js/summarizer.js",
        include_str!("../../static/js/summarizer.js"),
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

/// Token embedded in the source of any static module that imports another
/// static module (e.g. `import … from './utils.js?v=__RDRS_ASSET_VERSION__'`).
/// It is substituted at serve time with the current build version so the nested
/// import resolves to a `?v=`-stamped URL, exactly like the top-level `<script>`
/// tags in the templates. Without it, ES-module imports request bare,
/// unversioned URLs that never change and so are cached forever under the
/// `immutable` header — a stale `utils.js` then silently breaks every module
/// that imports it (the `debounce` regression).
const ASSET_VERSION_PLACEHOLDER: &str = "__RDRS_ASSET_VERSION__";

fn content_type_for(path: &str) -> &'static str {
    match std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
    {
        Some("css") => "text/css; charset=utf-8",
        Some("woff2") => "font/woff2",
        _ => "application/javascript",
    }
}

/// Shared with `handlers::favicon`, which embeds its images the same way and
/// so has exactly the same invalidation problem across an upgrade.
pub(crate) fn cache_control_for() -> &'static str {
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
            // Stamp nested-import URLs with the build version so they cache-bust
            // across deploys. No-op for assets without the placeholder.
            content.replace(ASSET_VERSION_PLACEHOLDER, crate::GIT_VERSION),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn body_of(path: &str) -> String {
        let resp = serve(Path(path.to_string())).await;
        let bytes = axum::body::to_bytes(resp.into_response().into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// Nested ES-module imports resolve to bare URLs (`/static/js/utils.js`)
    /// that carry no `?v=` cache-buster, so under the 1-year `immutable` header
    /// a browser keeps a stale copy across deploys — the exact trap that broke
    /// `app.js`'s `debounce` import. Every importer of `utils.js` must request a
    /// version-stamped URL so a new release invalidates the cache.
    #[tokio::test]
    async fn js_nested_imports_are_version_busted() {
        let expected = format!("utils.js?v={}", crate::GIT_VERSION);
        for path in [
            "js/app.js",
            "js/components/rdrs-sidebar.js",
            "js/passkey.js",
        ] {
            let body = body_of(path).await;
            assert!(
                !body.contains(ASSET_VERSION_PLACEHOLDER),
                "{path}: placeholder was not substituted"
            );
            assert!(
                body.contains(&expected),
                "{path}: expected a version-stamped utils.js import ({expected})"
            );
            assert!(
                !body.contains("utils.js'") && !body.contains("utils.js\""),
                "{path}: still imports utils.js without a ?v= cache-buster"
            );
        }
    }

    /// Every `/static/js/*.js` a template references must be registered in
    /// `FILES`, or the browser gets a 404 for it. Assets are embedded by an
    /// explicit list here, so adding a `<script src>` to a template without a
    /// matching `FILES` entry silently ships a broken page — e.g. an unregistered
    /// `csrf.js` 404s, `window.fetch` is never patched, and every token-bearing
    /// POST starts failing. This walks the templates and fails on any referenced
    /// script the serve table cannot satisfy.
    #[tokio::test]
    async fn every_template_referenced_script_is_served() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/templates");
        let mut stack = vec![std::path::PathBuf::from(root)];
        let mut refs: Vec<String> = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let html = std::fs::read_to_string(&path).unwrap();
                for (i, _) in html.match_indices("/static/js/") {
                    let tail = &html[i + "/static/".len()..];
                    let name: String = tail
                        .chars()
                        .take_while(|c| !matches!(c, '"' | '\'' | '?' | ' ' | '>' | '\n'))
                        .collect();
                    if std::path::Path::new(&name)
                        .extension()
                        .and_then(|e| e.to_str())
                        == Some("js")
                    {
                        refs.push(name);
                    }
                }
            }
        }
        assert!(
            refs.iter().any(|r| r == "js/csrf.js"),
            "sanity: expected to find at least the csrf.js reference while scanning templates"
        );
        for name in &refs {
            assert!(
                FILES.iter().any(|(n, _)| n == name),
                "template references /static/{name} but it is not registered in FILES (404)"
            );
        }
    }
}
