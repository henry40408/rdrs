//! `PUT /api/user/client-info`: facts only the browser knows. The server never
//! stores them; it compares each with the account's settings and answers with
//! the hints the page should show. The browser decides when to report
//! (`static/js/client-info.js`), so a settled mismatch is not raised again.
//!
//! The hints travel in the response, not in a flash cookie: this request is
//! sent after the page has loaded, and a cookie set then would be consumed by
//! whichever request reads flashes next, often a fragment swap that shows none.
//!
//! To report another fact: add an `Option` field to [`ClientInfo`] and one
//! `*_hint` function, and call it from [`put_client_info`].

use axum::{Json, extract::State};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use url::form_urlencoded::byte_serialize;

use crate::AppState;
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::middleware::AuthUser;
use crate::models::user_settings;
use crate::utils::timezone;

/// Every field is optional so an older page and a newer server (or the
/// reverse) still understand each other.
#[derive(Debug, Deserialize)]
pub struct ClientInfo {
    /// The browser's IANA timezone name.
    pub timezone: Option<String>,
}

/// A notice for the page's flash area, with one link.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Hint {
    pub message: String,
    pub action_label: String,
    /// Built here, never taken from the request.
    pub action_href: String,
}

#[derive(Debug, Serialize)]
pub struct ClientInfoReply {
    pub hints: Vec<Hint>,
}

pub async fn put_client_info(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Json(info): Json<ClientInfo>,
) -> AppResult<Json<ClientInfoReply>> {
    // The browser belongs to the admin, not to the account being viewed.
    if auth_user.session.is_masquerading() {
        return Ok(Json(ClientInfoReply { hints: Vec::new() }));
    }

    let mut hints = Vec::new();
    if let Some(name) = &info.timezone {
        hints.extend(timezone_hint(&state.db, auth_user.user.id, name).await?);
    }
    Ok(Json(ClientInfoReply { hints }))
}

/// Suggests the browser's timezone when it cuts days differently from the
/// stored one. It only suggests: the settings form applies the change.
async fn timezone_hint(db: &Db, user_id: i64, browser_timezone: &str) -> AppResult<Option<Hint>> {
    let browser_timezone = browser_timezone.trim();
    let Some(browser) = timezone::parse(browser_timezone) else {
        return Err(AppError::Validation(format!(
            "\"{browser_timezone}\" is not a timezone name"
        )));
    };
    let stored = user_settings::get_timezone(db, user_id).await?;
    if timezone::same_rules(browser, stored, Utc::now()) {
        return Ok(None);
    }

    let encoded: String = byte_serialize(browser.name().as_bytes()).collect();
    Ok(Some(Hint {
        message: format!(
            "Your browser's timezone is {}, but Statistics cuts days in {}.",
            browser.name(),
            stored.name()
        ),
        action_label: "Review in Preferences".to_string(),
        action_href: format!("/user-settings?timezone={encoded}"),
    }))
}
