use axum::{extract::State, Form, Json};
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::models::{category, entry, feed};
use crate::AppState;

use super::auth::GReaderUser;
use super::types::{item_id_to_entry_id, StreamId, Tag, TagListResponse};

/// `GET /reader/api/0/tag/list`
pub async fn tag_list(
    auth: GReaderUser,
    State(state): State<AppState>,
) -> AppResult<Json<TagListResponse>> {
    let user_id = auth.user.id;

    let tags = state
        .db
        .read_user(move |conn| {
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

            let categories = category::list_by_user(conn, user_id)?;
            for cat in categories {
                tags.push(Tag {
                    id: format!("user/-/label/{}", cat.name),
                    tag_type: Some("folder".to_string()),
                    sort_id: Some(format!("{:08x}", cat.id)),
                });
            }

            Ok::<_, AppError>(tags)
        })
        .await??;

    Ok(Json(TagListResponse { tags }))
}

// --- edit-tag ---

/// Form data for edit-tag. Supports multiple `i` parameters via Vec.
#[derive(Debug, Deserialize)]
pub struct EditTagForm {
    /// Add tag
    pub a: Option<String>,
    /// Remove tag
    pub r: Option<String>,
    /// POST token
    #[serde(rename = "T")]
    pub token: Option<String>,
}

/// `POST /reader/api/0/edit-tag`
///
/// Supports batch operations via multiple `i=` parameters.
pub async fn edit_tag(
    auth: GReaderUser,
    State(state): State<AppState>,
    Form(raw_form): Form<Vec<(String, String)>>,
) -> AppResult<String> {
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

    state
        .db
        .user(move |conn| {
            // Batch mark-as-read: verify ownership, then use efficient bulk operation
            if matches!(add_stream, Some(StreamId::Read)) {
                // Verify all entries belong to the user
                let found = entry::find_by_ids_with_feed(conn, user_id, &entry_ids)?;
                if found.len() != entry_ids.len() {
                    return Err(AppError::EntryNotFound);
                }
                entry::mark_read_by_ids(conn, user_id, &entry_ids)?;
                return Ok(());
            }

            // Other operations: process individually (no bulk functions available)
            for entry_id in &entry_ids {
                // Verify ownership
                let ewf =
                    entry::find_by_id_with_feed(conn, *entry_id)?.ok_or(AppError::EntryNotFound)?;
                category::find_by_id_and_user(conn, ewf.category_id, user_id)?
                    .ok_or(AppError::EntryNotFound)?;

                // Apply add tag
                if let Some(ref stream) = add_stream {
                    match stream {
                        StreamId::Starred => {
                            entry::star_entry(conn, *entry_id)?;
                        }
                        StreamId::KeptUnread => {
                            entry::mark_as_unread(conn, *entry_id)?;
                        }
                        _ => {}
                    }
                }

                // Apply remove tag
                if let Some(ref stream) = remove_stream {
                    match stream {
                        StreamId::Read => {
                            entry::mark_as_unread(conn, *entry_id)?;
                        }
                        StreamId::Starred => {
                            entry::unstar_entry(conn, *entry_id)?;
                        }
                        _ => {}
                    }
                }
            }

            Ok::<_, AppError>(())
        })
        .await??;

    state.sidebar_cache.bust(user_id);
    Ok("OK".to_string())
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
) -> AppResult<String> {
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

    state
        .db
        .user(move |conn| {
            match stream_id {
                StreamId::ReadingList => {
                    entry::mark_all_read_by_user(conn, user_id, older_than_days)?;
                }
                StreamId::Feed(url) => {
                    let f = feed::find_by_url_for_user(conn, &url, user_id)?
                        .ok_or(AppError::FeedNotFound)?;
                    entry::mark_all_read_by_feed(conn, f.id, older_than_days)?;
                }
                StreamId::Label(name) => {
                    let cat = category::find_by_name_and_user(conn, &name, user_id)?
                        .ok_or(AppError::CategoryNotFound)?;
                    entry::mark_all_read_by_category(conn, cat.id, older_than_days)?;
                }
                _ => {
                    return Err(AppError::Validation(
                        "Invalid stream for mark-all-as-read".into(),
                    ));
                }
            }

            Ok::<_, AppError>(())
        })
        .await??;

    state.sidebar_cache.bust(user_id);
    Ok("OK".to_string())
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
    let label_name = match stream_id {
        StreamId::Label(name) => name,
        _ => return Err(AppError::Validation("Can only disable label tags".into())),
    };

    let user_id = auth.user.id;

    state
        .db
        .user(move |conn| {
            let cat = category::find_by_name_and_user(conn, &label_name, user_id)?
                .ok_or(AppError::CategoryNotFound)?;
            category::delete_category(conn, cat.id, user_id)?;
            Ok::<_, AppError>(())
        })
        .await??;

    state.sidebar_cache.bust(user_id);
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

    let old_name = match source {
        StreamId::Label(name) => name,
        _ => return Err(AppError::Validation("Can only rename label tags".into())),
    };

    let new_name = match dest {
        StreamId::Label(name) => name,
        _ => {
            return Err(AppError::Validation(
                "Destination must be a label tag".into(),
            ))
        }
    };

    let user_id = auth.user.id;

    state
        .db
        .user(move |conn| {
            if old_name == new_name {
                // Same name: create the category if it doesn't exist (idempotent)
                if category::find_by_name_and_user(conn, &old_name, user_id)?.is_none() {
                    category::create_category(conn, user_id, &new_name)?;
                }
            } else {
                let cat = category::find_by_name_and_user(conn, &old_name, user_id)?
                    .ok_or(AppError::CategoryNotFound)?;
                category::update_name(conn, cat.id, user_id, &new_name)?;
            }
            Ok::<_, AppError>(())
        })
        .await??;

    state.sidebar_cache.bust(user_id);
    Ok("OK".to_string())
}
