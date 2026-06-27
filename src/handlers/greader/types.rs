use std::fmt;

use serde::Serialize;

use crate::error::{AppError, AppResult};

// --- Stream ID ---

/// Represents a Google Reader stream identifier.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamId {
    /// `user/-/state/com.google/reading-list` — all items
    ReadingList,
    /// `user/-/state/com.google/read` — read items
    Read,
    /// `user/-/state/com.google/starred` — starred items
    Starred,
    /// `user/-/state/com.google/kept-unread` — kept unread items
    KeptUnread,
    /// `user/-/label/<name>` — a user category/label
    Label(String),
    /// `feed/<url>` — a specific feed by URL
    Feed(String),
}

impl StreamId {
    /// Parse a stream ID string into a `StreamId`.
    pub fn parse(s: &str) -> AppResult<Self> {
        if let Some(url) = s.strip_prefix("feed/") {
            if url.is_empty() {
                return Err(AppError::Validation("Empty feed URL in stream ID".into()));
            }
            return Ok(StreamId::Feed(url.to_string()));
        }

        // Normalize user ID: accept both "user/-/" and "user/<numeric>/"
        let normalized = if let Some(after_user) = s.strip_prefix("user/") {
            if let Some(rest) = s.strip_prefix("user/-/") {
                rest
            } else if let Some(pos) = after_user.find('/') {
                &after_user[pos + 1..]
            } else {
                return Err(AppError::Validation(format!("Invalid stream ID: {}", s)));
            }
        } else {
            return Err(AppError::Validation(format!("Invalid stream ID: {}", s)));
        };

        match normalized {
            "state/com.google/reading-list" => Ok(StreamId::ReadingList),
            "state/com.google/read" => Ok(StreamId::Read),
            "state/com.google/starred" => Ok(StreamId::Starred),
            "state/com.google/kept-unread" => Ok(StreamId::KeptUnread),
            _ if normalized.starts_with("label/") => {
                let name = &normalized[6..];
                if name.is_empty() {
                    return Err(AppError::Validation("Empty label name".into()));
                }
                Ok(StreamId::Label(name.to_string()))
            }
            _ => Err(AppError::Validation(format!("Unknown stream ID: {}", s))),
        }
    }
}

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StreamId::ReadingList => write!(f, "user/-/state/com.google/reading-list"),
            StreamId::Read => write!(f, "user/-/state/com.google/read"),
            StreamId::Starred => write!(f, "user/-/state/com.google/starred"),
            StreamId::KeptUnread => write!(f, "user/-/state/com.google/kept-unread"),
            StreamId::Label(name) => write!(f, "user/-/label/{}", name),
            StreamId::Feed(url) => write!(f, "feed/{}", url),
        }
    }
}

// --- Item ID ---

const ITEM_ID_PREFIX: &str = "tag:google.com,2005:reader/item/";

/// Convert an internal entry ID (i64) to a Google Reader item ID string.
pub fn entry_id_to_item_id(id: i64) -> String {
    format!("{}{:016x}", ITEM_ID_PREFIX, id)
}

/// Parse a Google Reader item ID string back to an internal entry ID (i64).
/// Accepts both long format (`tag:google.com,2005:reader/item/<hex>`) and short format (plain number).
pub fn item_id_to_entry_id(s: &str) -> AppResult<i64> {
    // Long format
    if let Some(hex) = s.strip_prefix(ITEM_ID_PREFIX) {
        let hex = hex.trim_start_matches('0');
        if hex.is_empty() {
            return Ok(0);
        }
        return i64::from_str_radix(hex, 16)
            .map_err(|_e| AppError::Validation(format!("Invalid item ID: {}", s)));
    }

    // Short format: try decimal first, then hex (some clients send bare hex like "0000000000005da1")
    if let Ok(id) = s.parse::<i64>() {
        return Ok(id);
    }

    let hex = s.trim_start_matches('0');
    if hex.is_empty() {
        return Ok(0);
    }
    i64::from_str_radix(hex, 16)
        .map_err(|_e| AppError::Validation(format!("Invalid item ID: {}", s)))
}

// --- Shared response types ---

/// Standard Google Reader item entry in stream/contents response.
/// Includes RDRS extension fields (prefixed with `_`) for Web UI use.
/// Third-party `GReader` clients safely ignore unknown fields.
#[derive(Debug, Serialize)]
pub struct GReaderItem {
    pub id: String,
    pub published: i64,
    pub updated: i64,
    #[serde(rename = "crawlTimeMsec")]
    pub crawl_time_msec: String,
    #[serde(rename = "timestampUsec")]
    pub timestamp_usec: String,
    pub title: String,
    pub categories: Vec<String>,
    pub summary: GReaderContent,
    pub canonical: Vec<GReaderLink>,
    pub alternate: Vec<GReaderAlternateLink>,
    pub author: String,
    pub origin: GReaderOrigin,

    // RDRS extension fields for Web UI (GReader clients ignore these)
    #[serde(rename = "_entryId")]
    pub entry_id: i64,
    #[serde(rename = "_feedId")]
    pub feed_id: i64,
    #[serde(rename = "_categoryId")]
    pub category_id: i64,
    #[serde(rename = "_categoryName")]
    pub category_name: String,
    #[serde(rename = "_feedHasIcon")]
    pub feed_has_icon: bool,
    #[serde(rename = "_readAt")]
    pub read_at: Option<String>,
    #[serde(rename = "_starredAt")]
    pub starred_at: Option<String>,
    #[serde(rename = "_publishedAt")]
    pub published_at: Option<String>,
    #[serde(rename = "_content")]
    pub content: Option<String>,
    #[serde(rename = "_summaryStatus", skip_serializing_if = "Option::is_none")]
    pub summary_status: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GReaderContent {
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct GReaderLink {
    pub href: String,
}

#[derive(Debug, Serialize)]
pub struct GReaderAlternateLink {
    pub href: String,
    #[serde(rename = "type")]
    pub link_type: String,
}

#[derive(Debug, Serialize)]
pub struct GReaderOrigin {
    #[serde(rename = "streamId")]
    pub stream_id: String,
    pub title: String,
    #[serde(rename = "htmlUrl")]
    pub html_url: String,
}

/// Response for `stream/contents`.
#[derive(Debug, Serialize)]
pub struct StreamContentsResponse {
    pub id: String,
    pub updated: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation: Option<String>,
    pub items: Vec<GReaderItem>,
}

/// Response for `stream/items/ids`.
#[derive(Debug, Serialize)]
pub struct StreamItemIdsResponse {
    #[serde(rename = "itemRefs")]
    pub item_refs: Vec<ItemRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ItemRef {
    pub id: String,
    #[serde(rename = "timestampUsec")]
    pub timestamp_usec: String,
}

/// Response for `unread-count`.
#[derive(Debug, Serialize)]
pub struct UnreadCountResponse {
    pub max: i64,
    pub unreadcounts: Vec<UnreadCount>,
}

#[derive(Debug, Serialize)]
pub struct UnreadCount {
    pub id: String,
    pub count: i64,
    #[serde(rename = "newestItemTimestampUsec")]
    pub newest_item_timestamp_usec: String,
}

/// Response for `subscription/list`.
#[derive(Debug, Serialize)]
pub struct SubscriptionListResponse {
    pub subscriptions: Vec<Subscription>,
}

#[derive(Debug, Serialize)]
pub struct Subscription {
    pub id: String,
    pub title: String,
    pub categories: Vec<SubscriptionCategory>,
    #[serde(rename = "sortid")]
    pub sort_id: String,
    #[serde(rename = "htmlUrl")]
    pub html_url: String,
    pub url: String,
    #[serde(rename = "iconUrl")]
    pub icon_url: String,

    // RDRS extension fields (GReader clients ignore these)
    #[serde(rename = "_feedId")]
    pub feed_id: i64,
    #[serde(rename = "_fetchError", skip_serializing_if = "Option::is_none")]
    pub fetch_error: Option<String>,
    #[serde(rename = "_description", skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "_customUserAgent", skip_serializing_if = "Option::is_none")]
    pub custom_user_agent: Option<String>,
    #[serde(rename = "_http2Disabled")]
    pub http2_disabled: bool,
    #[serde(rename = "_customReferrer", skip_serializing_if = "Option::is_none")]
    pub custom_referrer: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SubscriptionCategory {
    pub id: String,
    pub label: String,
}

/// Response for `tag/list`.
#[derive(Debug, Serialize)]
pub struct TagListResponse {
    pub tags: Vec<Tag>,
}

#[derive(Debug, Serialize)]
pub struct Tag {
    pub id: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub tag_type: Option<String>,
    #[serde(rename = "sortid", skip_serializing_if = "Option::is_none")]
    pub sort_id: Option<String>,
}

/// Response for `user-info`.
#[derive(Debug, Serialize)]
pub struct UserInfoResponse {
    #[serde(rename = "userId")]
    pub user_id: String,
    #[serde(rename = "userName")]
    pub user_name: String,
    #[serde(rename = "userProfileId")]
    pub user_profile_id: String,
    #[serde(rename = "userEmail")]
    pub user_email: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_id_parse_reading_list() {
        assert_eq!(
            StreamId::parse("user/-/state/com.google/reading-list").unwrap(),
            StreamId::ReadingList
        );
    }

    #[test]
    fn test_stream_id_parse_read() {
        assert_eq!(
            StreamId::parse("user/-/state/com.google/read").unwrap(),
            StreamId::Read
        );
    }

    #[test]
    fn test_stream_id_parse_starred() {
        assert_eq!(
            StreamId::parse("user/-/state/com.google/starred").unwrap(),
            StreamId::Starred
        );
    }

    #[test]
    fn test_stream_id_parse_label() {
        assert_eq!(
            StreamId::parse("user/-/label/Tech").unwrap(),
            StreamId::Label("Tech".to_string())
        );
    }

    #[test]
    fn test_stream_id_parse_feed() {
        assert_eq!(
            StreamId::parse("feed/https://example.com/feed.xml").unwrap(),
            StreamId::Feed("https://example.com/feed.xml".to_string())
        );
    }

    #[test]
    fn test_stream_id_parse_numeric_user() {
        assert_eq!(
            StreamId::parse("user/12345/state/com.google/reading-list").unwrap(),
            StreamId::ReadingList
        );
    }

    #[test]
    fn test_stream_id_display_roundtrip() {
        let ids = vec![
            StreamId::ReadingList,
            StreamId::Read,
            StreamId::Starred,
            StreamId::KeptUnread,
            StreamId::Label("Tech".to_string()),
            StreamId::Feed("https://example.com/feed.xml".to_string()),
        ];

        for id in ids {
            let s = id.to_string();
            assert_eq!(StreamId::parse(&s).unwrap(), id);
        }
    }

    #[test]
    fn test_entry_id_to_item_id() {
        assert_eq!(
            entry_id_to_item_id(1),
            "tag:google.com,2005:reader/item/0000000000000001"
        );
        assert_eq!(
            entry_id_to_item_id(255),
            "tag:google.com,2005:reader/item/00000000000000ff"
        );
    }

    #[test]
    fn test_item_id_to_entry_id_long() {
        assert_eq!(
            item_id_to_entry_id("tag:google.com,2005:reader/item/0000000000000001").unwrap(),
            1
        );
        assert_eq!(
            item_id_to_entry_id("tag:google.com,2005:reader/item/00000000000000ff").unwrap(),
            255
        );
    }

    #[test]
    fn test_item_id_to_entry_id_short() {
        assert_eq!(item_id_to_entry_id("1").unwrap(), 1);
        assert_eq!(item_id_to_entry_id("255").unwrap(), 255);
    }

    #[test]
    fn test_item_id_to_entry_id_bare_hex() {
        // Some clients send the hex part without the tag prefix
        assert_eq!(item_id_to_entry_id("0000000000005da1").unwrap(), 0x5da1);
        assert_eq!(item_id_to_entry_id("00000000000000ff").unwrap(), 255);
        assert_eq!(item_id_to_entry_id("0000000000000000").unwrap(), 0);
    }

    #[test]
    fn test_item_id_roundtrip() {
        for id in [0, 1, 42, 1000, i64::MAX] {
            let item_id = entry_id_to_item_id(id);
            assert_eq!(item_id_to_entry_id(&item_id).unwrap(), id);
        }
    }
}
