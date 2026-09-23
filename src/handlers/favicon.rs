use axum::{
    extract::RawQuery,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::handlers::static_assets::cache_control_for;

const FAVICON_ICO: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/favicon.ico"));
const FAVICON_SVG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/favicon.svg"));
const FAVICON_16: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/favicon-16x16.png"));
const FAVICON_32: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/favicon-32x32.png"));
const APPLE_TOUCH_ICON: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/apple-touch-icon.png"));

/// Unversioned requests: short and *not* `immutable`.
const UNVERSIONED_CACHE_CONTROL: &str = "public, max-age=3600";

/// Only `?v=`-stamped URLs are pinned `immutable`; bare well-known paths
/// (browser probes, iOS touch icon) have no URL that changes on upgrade.
fn cache_control_for_request(query: Option<&str>) -> &'static str {
    let versioned = query.is_some_and(|q| q.split('&').any(|param| param.starts_with("v=")));

    if versioned {
        cache_control_for()
    } else {
        UNVERSIONED_CACHE_CONTROL
    }
}

fn icon_response(
    bytes: &'static [u8],
    content_type: &'static str,
    query: Option<&str>,
) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache_control_for_request(query)),
        ],
        bytes,
    )
        .into_response()
}

pub async fn favicon_ico(RawQuery(query): RawQuery) -> Response {
    icon_response(FAVICON_ICO, "image/x-icon", query.as_deref())
}

pub async fn favicon_svg(RawQuery(query): RawQuery) -> Response {
    icon_response(FAVICON_SVG, "image/svg+xml", query.as_deref())
}

pub async fn favicon_16(RawQuery(query): RawQuery) -> Response {
    icon_response(FAVICON_16, "image/png", query.as_deref())
}

pub async fn favicon_32(RawQuery(query): RawQuery) -> Response {
    icon_response(FAVICON_32, "image/png", query.as_deref())
}

pub async fn apple_touch_icon(RawQuery(query): RawQuery) -> Response {
    icon_response(APPLE_TOUCH_ICON, "image/png", query.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_requests_get_the_long_lived_header() {
        assert_eq!(
            cache_control_for_request(Some("v=abc123")),
            cache_control_for()
        );
        // The stamp does not have to come first.
        assert_eq!(
            cache_control_for_request(Some("foo=1&v=abc123")),
            cache_control_for()
        );
    }

    #[test]
    fn unversioned_requests_get_a_short_ttl() {
        assert_eq!(cache_control_for_request(None), UNVERSIONED_CACHE_CONTROL);
        assert_eq!(
            cache_control_for_request(Some("")),
            UNVERSIONED_CACHE_CONTROL
        );
        assert_eq!(
            cache_control_for_request(Some("size=32")),
            UNVERSIONED_CACHE_CONTROL
        );
        // `v` must be the parameter name, not a substring of one.
        assert_eq!(
            cache_control_for_request(Some("rev=abc123")),
            UNVERSIONED_CACHE_CONTROL
        );
    }
}
