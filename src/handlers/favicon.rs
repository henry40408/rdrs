use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

// Embed generated favicon files at compile time
const FAVICON_ICO: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/favicon.ico"));
const FAVICON_SVG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/favicon.svg"));
const FAVICON_16: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/favicon-16x16.png"));
const FAVICON_32: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/favicon-32x32.png"));
const APPLE_TOUCH_ICON: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/apple-touch-icon.png"));

pub async fn favicon_ico() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/x-icon")],
        FAVICON_ICO,
    )
        .into_response()
}

pub async fn favicon_svg() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/svg+xml")],
        FAVICON_SVG,
    )
        .into_response()
}

pub async fn favicon_16() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/png")],
        FAVICON_16,
    )
        .into_response()
}

pub async fn favicon_32() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/png")],
        FAVICON_32,
    )
        .into_response()
}

pub async fn apple_touch_icon() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/png")],
        APPLE_TOUCH_ICON,
    )
        .into_response()
}
