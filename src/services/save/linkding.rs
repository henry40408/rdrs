use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{BookmarkData, SaveResult};
use crate::error::{AppError, AppResult};
use crate::services::http::{EXTERNAL_API_TIMEOUT, RetryConfig, send_with_retry_on_error};

/// Linkding service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkdingConfig {
    pub api_url: String,
    pub api_token: String,
}

impl LinkdingConfig {
    /// Check if the configuration is valid (both fields non-empty)
    pub fn is_configured(&self) -> bool {
        !self.api_url.is_empty() && !self.api_token.is_empty()
    }
}

/// Request body for Linkding API
#[derive(Debug, Clone, Serialize)]
struct LinkdingBookmarkRequest {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tag_names: Vec<String>,
}

/// Response from Linkding API
#[derive(Debug, Deserialize)]
struct LinkdingBookmarkResponse {
    id: i64,
}

/// Save a bookmark to Linkding
pub async fn save_to_linkding(
    config: &LinkdingConfig,
    bookmark: &BookmarkData,
) -> AppResult<SaveResult> {
    if !config.is_configured() {
        return Ok(SaveResult {
            success: false,
            service: "linkding".to_string(),
            message: "Linkding is not configured".to_string(),
            bookmark_url: None,
        });
    }

    let client = Client::builder()
        .timeout(EXTERNAL_API_TIMEOUT)
        .build()
        .map_err(|e| AppError::Internal(format!("Failed to build HTTP client: {}", e)))?;

    // Normalize API URL - ensure it ends with /api/bookmarks/
    let api_url = normalize_api_url(&config.api_url);

    let request_body = LinkdingBookmarkRequest {
        url: bookmark.url.clone(),
        title: bookmark.title.clone(),
        description: bookmark.description.clone(),
        tag_names: bookmark.tags.clone(),
    };

    let token = format!("Token {}", config.api_token);
    let response = send_with_retry_on_error(&RetryConfig::default(), || {
        client
            .post(&api_url)
            .header("Authorization", &token)
            .header("Content-Type", "application/json")
            .json(&request_body)
    })
    .await
    .map_err(|e| AppError::Internal(format!("Failed to connect to Linkding: {}", e)))?;

    let status = response.status();

    if status.is_success() {
        let body: LinkdingBookmarkResponse = response
            .json()
            .await
            .map_err(|e| AppError::Internal(format!("Failed to parse Linkding response: {}", e)))?;

        // Construct bookmark URL for the user to view
        let bookmark_url = construct_bookmark_url(&config.api_url, body.id);

        Ok(SaveResult {
            success: true,
            service: "linkding".to_string(),
            message: "Saved to Linkding".to_string(),
            bookmark_url: Some(bookmark_url),
        })
    } else {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());

        // Handle specific error codes
        let message = match status.as_u16() {
            400 => {
                // Check if it's a duplicate URL error
                if error_text.contains("already exists") || error_text.contains("unique") {
                    "Bookmark already exists in Linkding".to_string()
                } else {
                    format!("Bad request: {}", error_text)
                }
            }
            401 => "Invalid API token".to_string(),
            403 => "Access forbidden".to_string(),
            404 => "Linkding API endpoint not found".to_string(),
            _ => format!("Linkding error ({}): {}", status, error_text),
        };

        Ok(SaveResult {
            success: false,
            service: "linkding".to_string(),
            message,
            bookmark_url: None,
        })
    }
}

/// Normalize the API URL to ensure it points to the bookmarks endpoint
fn normalize_api_url(base_url: &str) -> String {
    let mut url = base_url.trim_end_matches('/').to_string();

    // Check if URL already contains /api
    if url.contains("/api") {
        // URL contains /api - add /bookmarks if it ends with /api
        if !url.ends_with("/bookmarks") && url.ends_with("/api") {
            url.push_str("/bookmarks");
        }
    } else {
        // URL doesn't contain /api, add /api/bookmarks
        url.push_str("/api/bookmarks");
    }

    // Ensure trailing slash
    if !url.ends_with('/') {
        url.push('/');
    }

    url
}

/// Construct the URL to view the bookmark in Linkding UI
fn construct_bookmark_url(base_url: &str, bookmark_id: i64) -> String {
    // Extract base URL (remove /api/... suffix)
    let base = if let Some(pos) = base_url.find("/api") {
        &base_url[..pos]
    } else {
        base_url.trim_end_matches('/')
    };

    format!("{}/bookmarks/{}", base, bookmark_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn save_success_returns_bookmark_url() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("authorization", "Token tok123"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"id": 42})))
            .expect(1)
            .mount(&server)
            .await;
        let config = LinkdingConfig {
            api_url: server.uri(),
            api_token: "tok123".into(),
        };
        let bookmark = BookmarkData {
            url: "https://example.com/post".into(),
            title: Some("Title".into()),
            description: None,
            tags: vec!["rust".into()],
        };
        let result = save_to_linkding(&config, &bookmark).await.unwrap();
        assert!(result.success);
        assert!(result.message.contains("Saved"));
    }

    #[tokio::test]
    async fn not_configured_short_circuits() {
        let config = LinkdingConfig {
            api_url: "".into(),
            api_token: "".into(),
        };
        let bookmark = BookmarkData {
            url: "https://example.com".into(),
            title: None,
            description: None,
            tags: vec![],
        };
        let result = save_to_linkding(&config, &bookmark).await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("not configured"));
    }

    #[tokio::test]
    async fn duplicate_returns_friendly_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(400).set_body_string(r#"{"url":["already exists"]}"#),
            )
            .mount(&server)
            .await;
        let config = LinkdingConfig {
            api_url: server.uri(),
            api_token: "tok".into(),
        };
        let bookmark = BookmarkData {
            url: "https://example.com".into(),
            title: None,
            description: None,
            tags: vec![],
        };
        let result = save_to_linkding(&config, &bookmark).await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("already exists"));
    }

    #[tokio::test]
    async fn bad_request_other() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad field"))
            .mount(&server)
            .await;
        let config = LinkdingConfig {
            api_url: server.uri(),
            api_token: "tok".into(),
        };
        let bookmark = BookmarkData {
            url: "https://example.com".into(),
            title: None,
            description: None,
            tags: vec![],
        };
        let result = save_to_linkding(&config, &bookmark).await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("Bad request"));
    }

    #[tokio::test]
    async fn invalid_token_401() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let config = LinkdingConfig {
            api_url: server.uri(),
            api_token: "bad-token".into(),
        };
        let bookmark = BookmarkData {
            url: "https://example.com".into(),
            title: None,
            description: None,
            tags: vec![],
        };
        let result = save_to_linkding(&config, &bookmark).await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("Invalid API token"));
    }

    #[tokio::test]
    async fn forbidden_403() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let config = LinkdingConfig {
            api_url: server.uri(),
            api_token: "tok".into(),
        };
        let bookmark = BookmarkData {
            url: "https://example.com".into(),
            title: None,
            description: None,
            tags: vec![],
        };
        let result = save_to_linkding(&config, &bookmark).await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("forbidden"));
    }

    #[tokio::test]
    async fn not_found_404() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let config = LinkdingConfig {
            api_url: server.uri(),
            api_token: "tok".into(),
        };
        let bookmark = BookmarkData {
            url: "https://example.com".into(),
            title: None,
            description: None,
            tags: vec![],
        };
        let result = save_to_linkding(&config, &bookmark).await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("endpoint not found"));
    }

    #[tokio::test]
    async fn server_error_500() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;
        let config = LinkdingConfig {
            api_url: server.uri(),
            api_token: "tok".into(),
        };
        let bookmark = BookmarkData {
            url: "https://example.com".into(),
            title: None,
            description: None,
            tags: vec![],
        };
        let result = save_to_linkding(&config, &bookmark).await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("Linkding error (500"));
    }

    #[tokio::test]
    async fn malformed_json_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201).set_body_string("{not json"))
            .mount(&server)
            .await;
        let config = LinkdingConfig {
            api_url: server.uri(),
            api_token: "tok".into(),
        };
        let bookmark = BookmarkData {
            url: "https://example.com".into(),
            title: None,
            description: None,
            tags: vec![],
        };
        let result = save_to_linkding(&config, &bookmark).await;
        assert!(matches!(result, Err(AppError::Internal(_))));
    }

    #[tokio::test]
    async fn omits_empty_optional_fields() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wiremock::matchers::body_json(
                serde_json::json!({"url": "https://x/"}),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"id": 1})))
            .mount(&server)
            .await;
        let config = LinkdingConfig {
            api_url: server.uri(),
            api_token: "tok".into(),
        };
        let bookmark = BookmarkData {
            url: "https://x/".into(),
            title: None,
            description: None,
            tags: vec![],
        };
        let result = save_to_linkding(&config, &bookmark).await.unwrap();
        assert!(result.success);
    }

    #[test]
    fn test_normalize_api_url() {
        // Base URL without /api/
        assert_eq!(
            normalize_api_url("https://linkding.example.com"),
            "https://linkding.example.com/api/bookmarks/"
        );

        // Base URL with trailing slash
        assert_eq!(
            normalize_api_url("https://linkding.example.com/"),
            "https://linkding.example.com/api/bookmarks/"
        );

        // URL with /api/ but no /bookmarks/
        assert_eq!(
            normalize_api_url("https://linkding.example.com/api"),
            "https://linkding.example.com/api/bookmarks/"
        );

        // URL with /api/bookmarks
        assert_eq!(
            normalize_api_url("https://linkding.example.com/api/bookmarks"),
            "https://linkding.example.com/api/bookmarks/"
        );

        // URL already complete
        assert_eq!(
            normalize_api_url("https://linkding.example.com/api/bookmarks/"),
            "https://linkding.example.com/api/bookmarks/"
        );
    }

    #[test]
    fn test_construct_bookmark_url() {
        assert_eq!(
            construct_bookmark_url("https://linkding.example.com", 123),
            "https://linkding.example.com/bookmarks/123"
        );

        assert_eq!(
            construct_bookmark_url("https://linkding.example.com/api/bookmarks/", 456),
            "https://linkding.example.com/bookmarks/456"
        );
    }

    #[test]
    fn test_linkding_config_is_configured() {
        let config = LinkdingConfig {
            api_url: "https://linkding.example.com".to_string(),
            api_token: "abc123".to_string(),
        };
        assert!(config.is_configured());

        let empty_url = LinkdingConfig {
            api_url: "".to_string(),
            api_token: "abc123".to_string(),
        };
        assert!(!empty_url.is_configured());

        let empty_token = LinkdingConfig {
            api_url: "https://linkding.example.com".to_string(),
            api_token: "".to_string(),
        };
        assert!(!empty_token.is_configured());
    }
}
