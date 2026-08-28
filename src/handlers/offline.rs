//! The sync ledger behind offline reading.
//!
//! The client cannot decide on its own what to keep: only the server knows
//! which entries are unread, which are starred and how large a budget the
//! reader consented to. This module answers that question and nothing else —
//! the articles themselves are fetched through the ordinary
//! `GET /entries/{id}/fragment` route, so there is exactly one renderer for a
//! reading pane whether it is being displayed or stored.

use axum::{Json, extract::State};
use serde::Serialize;

use crate::AppState;
use crate::error::AppResult;
use crate::middleware::auth::AuthUser;
use crate::models::{entry, user_settings};
use crate::secret;

/// One entry the client should hold offline.
#[derive(Debug, Serialize)]
pub struct OfflineEntryDto {
    pub id: i64,
    /// Validity token for the cached fragment, not merely a timestamp: every
    /// write that can change the rendered pane — content, full content, read
    /// and star state — bumps `entry.updated_at`, so a client that re-fetches
    /// whenever this moves is exactly as fresh as the markup requires and no
    /// chattier.
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct OfflineManifest {
    /// Opaque name for this reader's cache. The client wipes and re-syncs when
    /// it changes, which is what stops one account's articles from surviving a
    /// sign-out into the next account on a shared device.
    pub cache_key: String,
    /// The reader's budget, echoed back so the client can tell "offline reading
    /// is off" from "on, but nothing qualifies yet" — the first must clear the
    /// cache, the second must leave it alone.
    pub keep: i64,
    pub entries: Vec<OfflineEntryDto>,
}

/// `GET /api/offline/manifest` — what this reader's browser should be holding.
///
/// Deliberately thin: ids and validity tokens, no titles or content. The client
/// already has (or is about to fetch) the markup, and a payload that repeated
/// it would be a second copy of the reader's data crossing the wire on every
/// sync, cached by nothing and useful to no one.
pub async fn manifest(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<OfflineManifest>> {
    let user_id = auth_user.user.id;
    let keep = user_settings::get_offline_keep(&state.db, user_id).await?;
    let entries = entry::list_offline_set(&state.db, user_id, keep)
        .await?
        .into_iter()
        .map(|e| OfflineEntryDto {
            id: e.entry.id,
            updated_at: e.entry.updated_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(OfflineManifest {
        cache_key: secret::offline_id(&state.config.secret, user_id),
        keep,
        entries,
    }))
}
