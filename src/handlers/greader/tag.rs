use axum::{Form, Json, extract::State};
use serde::Deserialize;

use crate::AppState;
use crate::error::{AppError, AppResult};
use crate::models::{category, entry, feed};

use super::auth::GReaderUser;
use super::types::{StreamId, Tag, TagListResponse, item_id_to_entry_id};

/// `GET /reader/api/0/tag/list`
pub async fn tag_list(
    auth: GReaderUser,
    State(state): State<AppState>,
) -> AppResult<Json<TagListResponse>> {
    let user_id = auth.user.id;

    let mut tags = vec![
        Tag {
            id: "user/-/state/com.google/reading-list".to_string(),
            tag_type: Some("state".to_string()),
            sort_id: None,
        },
        Tag {
            id: "user/-/state/com.google/read".to_string(),
            tag_type: Some("state".to_string()),
            sort_id: None,
        },
        Tag {
            id: "user/-/state/com.google/starred".to_string(),
            tag_type: Some("state".to_string()),
            sort_id: None,
        },
        Tag {
            id: "user/-/state/com.google/kept-unread".to_string(),
            tag_type: Some("state".to_string()),
            sort_id: None,
        },
    ];

    let categories = category::list_by_user(&state.db, user_id).await?;
    for cat in categories {
        tags.push(Tag {
            id: format!("user/-/label/{}", cat.name),
            tag_type: Some("folder".to_string()),
            sort_id: Some(format!("{:08x}", cat.id)),
        });
    }

    Ok(Json(TagListResponse { tags }))
}

// --- edit-tag ---

/// `POST /reader/api/0/edit-tag`
///
/// Supports batch operations via multiple `i=` parameters.
pub async fn edit_tag(
    auth: GReaderUser,
    State(state): State<AppState>,
    Form(raw_form): Form<Vec<(String, String)>>,
) -> AppResult<([(&'static str, String); 1], String)> {
    // Extract repeated `i` params and other fields
    let mut item_ids: Vec<String> = Vec::new();
    let mut add_tag: Option<String> = None;
    let mut remove_tag: Option<String> = None;
    let mut _token: Option<String> = None;

    for (key, value) in &raw_form {
        match key.as_str() {
            "i" => item_ids.push(value.clone()),
            "a" => add_tag = Some(value.clone()),
            "r" => remove_tag = Some(value.clone()),
            "T" => _token = Some(value.clone()),
            _ => {}
        }
    }

    if item_ids.is_empty() {
        return Err(AppError::Validation("No item IDs provided".into()));
    }

    // Parse item IDs
    let entry_ids: Vec<i64> = item_ids
        .iter()
        .map(|s| item_id_to_entry_id(s))
        .collect::<AppResult<Vec<_>>>()?;

    let user_id = auth.user.id;

    // Determine the operation based on add/remove tags
    let add_stream = add_tag.as_deref().map(StreamId::parse).transpose()?;
    let remove_stream = remove_tag.as_deref().map(StreamId::parse).transpose()?;

    // Batch mark-as-read: verify ownership, then use efficient bulk operation.
    //
    // The count these bulk updates return is the number of rows that actually
    // changed, which is what gets reported to the user — not `entry_ids.len()`.
    // Re-marking 40 already-read entries changed nothing, and saying "marked
    // 40" there would be a lie the UI used to tell by counting DOM rows.
    let affected = if matches!(add_stream, Some(StreamId::Read)) {
        // Verify all entries belong to the user
        let found = entry::find_by_ids_with_feed(&state.db, user_id, &entry_ids).await?;
        if found.len() != entry_ids.len() {
            return Err(AppError::EntryNotFound);
        }
        entry::mark_read_by_ids(&state.db, user_id, &entry_ids).await?
    } else {
        // Other operations: verify ownership for all ids once, then apply the
        // tag changes as bulk UPDATEs inside a single transaction (instead of
        // a per-entry read + UPDATE + re-read loop, untransacted).
        let found = entry::find_by_ids_with_feed(&state.db, user_id, &entry_ids).await?;
        if found.len() != entry_ids.len() {
            return Err(AppError::EntryNotFound);
        }

        let mut tx = state.db.begin().await?;
        let mut changed = 0_i64;

        // Apply add tag
        if let Some(ref stream) = add_stream {
            match stream {
                StreamId::Starred => {
                    changed += entry::star_by_ids_tx(&mut tx, user_id, &entry_ids).await?;
                }
                StreamId::KeptUnread => {
                    changed += entry::mark_unread_by_ids_tx(&mut tx, user_id, &entry_ids).await?;
                }
                _ => {}
            }
        }

        // Apply remove tag
        if let Some(ref stream) = remove_stream {
            match stream {
                StreamId::Read => {
                    changed += entry::mark_unread_by_ids_tx(&mut tx, user_id, &entry_ids).await?;
                }
                StreamId::Starred => {
                    changed += entry::unstar_by_ids_tx(&mut tx, user_id, &entry_ids).await?;
                }
                _ => {}
            }
        }

        tx.commit().await?;
        changed
    };

    state.sidebar_cache.bust(user_id);
    // Busting alone only helps the next request that renders chrome. A GReader
    // client's write never passes through the browser, so without this an open
    // tab keeps showing the pre-change counts until something else reloads it.
    state.events.emit_sidebar(user_id);
    Ok(super::ok_with_affected(affected))
}

// --- mark-all-as-read ---

#[derive(Debug, Deserialize)]
pub struct MarkAllReadForm {
    /// Stream ID to mark as read
    pub s: String,
    /// Timestamp in microseconds — only mark items older than this
    pub ts: Option<String>,
    /// POST token
    #[serde(rename = "T")]
    pub token: Option<String>,
}

/// `POST /reader/api/0/mark-all-as-read`
pub async fn mark_all_as_read(
    auth: GReaderUser,
    State(state): State<AppState>,
    Form(form): Form<MarkAllReadForm>,
) -> AppResult<([(&'static str, String); 1], String)> {
    let stream_id = StreamId::parse(&form.s)?;
    let user_id = auth.user.id;

    // Convert microsecond timestamp to older_than_days (approximate)
    let older_than_days = form.ts.as_ref().and_then(|ts_str| {
        let ts_usec: i64 = ts_str.parse().ok()?;
        let ts_secs = ts_usec / 1_000_000;
        let now = chrono::Utc::now().timestamp();
        let diff_secs = now - ts_secs;
        if diff_secs > 0 {
            Some(diff_secs / 86400)
        } else {
            None
        }
    });

    let affected = match stream_id {
        StreamId::ReadingList => {
            entry::mark_all_read_by_user(&state.db, user_id, older_than_days).await?
        }
        StreamId::Feed(url) => {
            let f = feed::find_by_url_for_user(&state.db, &url, user_id)
                .await?
                .ok_or(AppError::FeedNotFound)?;
            entry::mark_all_read_by_feed(&state.db, f.id, older_than_days).await?
        }
        StreamId::Label(name) => {
            let cat = category::find_by_name_and_user(&state.db, &name, user_id)
                .await?
                .ok_or(AppError::CategoryNotFound)?;
            entry::mark_all_read_by_category(&state.db, cat.id, older_than_days).await?
        }
        _ => {
            return Err(AppError::Validation(
                "Invalid stream for mark-all-as-read".into(),
            ));
        }
    };

    state.sidebar_cache.bust(user_id);
    // Busting alone only helps the next request that renders chrome. A GReader
    // client's write never passes through the browser, so without this an open
    // tab keeps showing the pre-change counts until something else reloads it.
    state.events.emit_sidebar(user_id);
    Ok(super::ok_with_affected(affected))
}

// --- disable-tag ---

#[derive(Debug, Deserialize)]
pub struct DisableTagForm {
    /// Tag to disable (e.g., "user/-/label/<name>")
    pub s: Option<String>,
    pub t: Option<String>,
    /// POST token
    #[serde(rename = "T")]
    pub token: Option<String>,
}

/// `POST /reader/api/0/disable-tag`
pub async fn disable_tag(
    auth: GReaderUser,
    State(state): State<AppState>,
    Form(form): Form<DisableTagForm>,
) -> AppResult<String> {
    let tag_str = form
        .s
        .or(form.t)
        .ok_or_else(|| AppError::Validation("Missing tag parameter (s or t)".into()))?;

    let stream_id = StreamId::parse(&tag_str)?;
    let StreamId::Label(label_name) = stream_id else {
        return Err(AppError::Validation("Can only disable label tags".into()));
    };

    let user_id = auth.user.id;

    let cat = category::find_by_name_and_user(&state.db, &label_name, user_id)
        .await?
        .ok_or(AppError::CategoryNotFound)?;
    category::delete_category(&state.db, cat.id, user_id).await?;

    state.sidebar_cache.bust(user_id);
    // Busting alone only helps the next request that renders chrome. A GReader
    // client's write never passes through the browser, so without this an open
    // tab keeps showing the pre-change counts until something else reloads it.
    state.events.emit_sidebar(user_id);
    Ok("OK".to_string())
}

// --- rename-tag ---

#[derive(Debug, Deserialize)]
pub struct RenameTagForm {
    /// Source tag
    pub s: Option<String>,
    pub t: Option<String>,
    /// Destination tag
    pub dest: Option<String>,
    /// POST token
    #[serde(rename = "T")]
    pub token: Option<String>,
}

/// `POST /reader/api/0/rename-tag`
pub async fn rename_tag(
    auth: GReaderUser,
    State(state): State<AppState>,
    Form(form): Form<RenameTagForm>,
) -> AppResult<String> {
    let source_str = form
        .s
        .or(form.t)
        .ok_or_else(|| AppError::Validation("Missing source tag (s or t)".into()))?;
    let dest_str = form
        .dest
        .ok_or_else(|| AppError::Validation("Missing destination tag (dest)".into()))?;

    let source = StreamId::parse(&source_str)?;
    let dest = StreamId::parse(&dest_str)?;

    let StreamId::Label(old_name) = source else {
        return Err(AppError::Validation("Can only rename label tags".into()));
    };

    let StreamId::Label(new_name) = dest else {
        return Err(AppError::Validation(
            "Destination must be a label tag".into(),
        ));
    };

    let user_id = auth.user.id;

    if old_name == new_name {
        // Same name: create the category if it doesn't exist (idempotent)
        if category::find_by_name_and_user(&state.db, &old_name, user_id)
            .await?
            .is_none()
        {
            category::create_category(&state.db, user_id, &new_name).await?;
        }
    } else {
        let cat = category::find_by_name_and_user(&state.db, &old_name, user_id)
            .await?
            .ok_or(AppError::CategoryNotFound)?;
        category::update_name(&state.db, cat.id, user_id, &new_name).await?;
    }

    state.sidebar_cache.bust(user_id);
    // Busting alone only helps the next request that renders chrome. A GReader
    // client's write never passes through the browser, so without this an open
    // tab keeps showing the pre-change counts until something else reloads it.
    state.events.emit_sidebar(user_id);
    Ok("OK".to_string())
}
