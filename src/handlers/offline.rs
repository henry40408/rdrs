//! The offline-reading manifest: which entries the client should keep. Articles
//! themselves come from `GET /entries/{id}/fragment`, the one pane renderer.

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
    /// Cache validity token: every write that changes the rendered pane bumps it.
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct OfflineManifest {
    /// Per-reader cache name; the client wipes on change so articles don't leak
    /// across accounts on a shared device.
    pub cache_key: String,
    /// Echoed budget: `0` (off) clears the cache; an empty list alone must not.
    pub keep: i64,
    pub entries: Vec<OfflineEntryDto>,
}

/// `GET /api/offline/manifest` — ids and validity tokens only, no content.
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
