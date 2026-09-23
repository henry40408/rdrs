//! `GET /p/{user_id}-{entry_id}-{sig}.gif` — the open-tracking pixel. Session-less
//! (external `GReader` clients carry no cookie); the path HMAC is the whole
//! authority — see [`secret::pixel_sig`].

use axum::{
    extract::{Path as AxumPath, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::{AppState, models::entry_open, secret, services::pixel::TRANSPARENT_GIF};

struct PixelToken {
    user_id: i64,
    entry_id: i64,
    sig: String,
}

/// Parse `{user_id}-{entry_id}-{sig}.gif`; `None` if malformed, answered like a
/// bad signature.
fn parse_token(raw: &str) -> Option<PixelToken> {
    let body = raw.strip_suffix(".gif")?;
    let mut parts = body.split('-');
    let user_id = parts.next()?.parse().ok()?;
    let entry_id = parts.next()?.parse().ok()?;
    let sig = parts.next()?.to_string();
    if parts.next().is_some() {
        return None;
    }
    Some(PixelToken {
        user_id,
        entry_id,
        sig,
    })
}

/// The response for every request, hit or miss: always 200 so probing can't
/// tell a valid token apart, and `no-store` so every open reports.
fn gif_response() -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/gif"),
            (header::CACHE_CONTROL, "no-store, max-age=0"),
        ],
        TRANSPARENT_GIF,
    )
        .into_response()
}

pub async fn tracking_pixel(
    State(state): State<AppState>,
    AxumPath(token): AxumPath<String>,
) -> Response {
    let Some(token) = parse_token(&token) else {
        return gif_response();
    };
    if !secret::verify_pixel_sig(
        &state.config.secret,
        token.user_id,
        token.entry_id,
        &token.sig,
    ) {
        return gif_response();
    }

    // Opt-in and ownership are enforced by the statement, so a stale token
    // after opt-out silently writes nothing.
    if let Err(e) = entry_open::record_open(&state.db, token.user_id, token.entry_id).await {
        tracing::warn!(event = "entry.record_open_failed", error = %e, "failed to record entry open");
    }

    gif_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_well_formed_token() {
        let t = parse_token("7-42-abcdef.gif").expect("valid token");
        assert_eq!(t.user_id, 7);
        assert_eq!(t.entry_id, 42);
        assert_eq!(t.sig, "abcdef");
    }

    #[test]
    fn rejects_malformed_tokens() {
        for raw in [
            "7-42-abcdef",     // no extension
            "7-42.gif",        // no signature
            "7-42-ab-cd.gif",  // one field too many
            "x-42-abcdef.gif", // non-numeric user
            "7-x-abcdef.gif",  // non-numeric entry
            "-7-42-abcdef.gif",
            ".gif",
            "",
        ] {
            assert!(parse_token(raw).is_none(), "{raw} should not parse");
        }
    }
}
