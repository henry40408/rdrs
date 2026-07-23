use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::services::http::{EXTERNAL_API_TIMEOUT, RetryConfig, send_with_retry_on_error};

/// Kagi Universal Summarizer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KagiConfig {
    /// Session token extracted from Kagi session link
    pub session_token: String,
    /// Target language for summary (optional, e.g., "ZH-HANT", "EN")
    #[serde(default)]
    pub language: Option<String>,
}

impl KagiConfig {
    /// Check if the configuration is valid
    pub fn is_configured(&self) -> bool {
        !self.session_token.is_empty()
    }
}

/// Output data from Kagi Summary API
#[derive(Debug, Deserialize)]
struct KagiOutputData {
    markdown: Option<String>,
}

/// Response from Kagi Summary API
#[derive(Debug, Deserialize)]
struct KagiSummaryResponse {
    output_data: Option<KagiOutputData>,
    error: Option<String>,
}

/// Result of summarization
#[derive(Debug, Clone, Serialize)]
pub struct SummarizeResult {
    pub success: bool,
    pub output_text: Option<String>,
    pub error: Option<String>,
    pub title: Option<String>,
}

const KAGI_API_BASE: &str = "https://kagi.com";

/// Summarize a URL using Kagi Universal Summarizer
pub async fn summarize_url(config: &KagiConfig, url: &str) -> AppResult<SummarizeResult> {
    // `RDRS_KAGI_API_BASE` env redirects the endpoint to a local stub for tests/E2E.
    // It is NEVER set in production — the default is the real Kagi host.
    let base = std::env::var("RDRS_KAGI_API_BASE").unwrap_or_else(|_| KAGI_API_BASE.to_string());
    summarize_url_with_base(&base, config, url).await
}

async fn summarize_url_with_base(
    base: &str,
    config: &KagiConfig,
    url: &str,
) -> AppResult<SummarizeResult> {
    if !config.is_configured() {
        return Ok(SummarizeResult {
            success: false,
            output_text: None,
            error: Some("Kagi is not configured".to_string()),
            title: None,
        });
    }

    let client = Client::builder()
        .timeout(EXTERNAL_API_TIMEOUT)
        .build()
        .map_err(|e| AppError::Internal(format!("Failed to build HTTP client: {e}")))?;

    // Build the API URL with query parameters
    let mut api_url = url::Url::parse(&format!("{base}/mother/summary_labs"))
        .map_err(|e| AppError::Internal(format!("Failed to parse Kagi API URL: {e}")))?;

    {
        let mut query = api_url.query_pairs_mut();
        query.append_pair("summary_type", "summary");
        query.append_pair("url", url);

        if let Some(lang) = &config.language
            && !lang.is_empty()
        {
            query.append_pair("target_language", lang);
        }
    }

    let api_url_str = api_url.to_string();
    let session_token = config.session_token.clone();
    let response = send_with_retry_on_error(&RetryConfig::default(), || {
        client
            .get(&api_url_str)
            .header("Authorization", &session_token)
            .header("Content-Type", "application/json")
    })
    .await
    .map_err(|e| AppError::Internal(format!("Failed to connect to Kagi: {e}")))?;

    let status = response.status();

    if status.is_success() {
        let body: KagiSummaryResponse = response
            .json()
            .await
            .map_err(|e| AppError::Internal(format!("Failed to parse Kagi response: {e}")))?;

        if let Some(error) = body.error {
            Ok(SummarizeResult {
                success: false,
                output_text: None,
                error: Some(error),
                title: None,
            })
        } else if let Some(markdown) = body.output_data.and_then(|d| d.markdown) {
            // Split off a leading "Title: <t>\n\n" prefix if present.
            let (title, cleaned) = if let Some(rest) = markdown.strip_prefix("Title: ") {
                match rest.find("\n\n") {
                    Some(pos) => (Some(rest[..pos].to_string()), rest[pos + 2..].to_string()),
                    None => (None, markdown),
                }
            } else {
                (None, markdown)
            };
            Ok(SummarizeResult {
                success: true,
                output_text: Some(cleaned),
                error: None,
                title,
            })
        } else {
            Ok(SummarizeResult {
                success: false,
                output_text: None,
                error: Some("No summary returned from Kagi".to_string()),
                title: None,
            })
        }
    } else {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());

        let message = match status.as_u16() {
            401 => "Invalid session token".to_string(),
            403 => "Access forbidden - check your Kagi subscription".to_string(),
            429 => "Rate limit exceeded - please try again later".to_string(),
            _ => format!("Kagi error ({status}): {error_text}"),
        };

        Ok(SummarizeResult {
            success: false,
            output_text: None,
            error: Some(message),
            title: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn test_kagi_config_is_configured() {
        let config = KagiConfig {
            session_token: "some_token".to_string(),
            language: Some("ZH-HANT".to_string()),
        };
        assert!(config.is_configured());

        let empty_token = KagiConfig {
            session_token: String::new(),
            language: None,
        };
        assert!(!empty_token.is_configured());
    }

    #[test]
    fn test_kagi_config_serialization() {
        let config = KagiConfig {
            session_token: "test_token".to_string(),
            language: Some("EN".to_string()),
        };

        let json = serde_json::to_string(&config).unwrap();
        let parsed: KagiConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.session_token, config.session_token);
        assert_eq!(parsed.language, config.language);
    }

    #[test]
    fn test_kagi_config_default_language() {
        let json = r#"{"session_token": "test"}"#;
        let config: KagiConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.session_token, "test");
        assert!(config.language.is_none());
    }

    #[tokio::test]
    async fn summarize_success_strips_title_prefix() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("authorization", "session-tok"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "output_data": {"markdown": "Title: Foo\n\nThe body."}
            })))
            .mount(&server)
            .await;
        let config = KagiConfig {
            session_token: "session-tok".into(),
            language: None,
        };
        let result = summarize_url_with_base(&server.uri(), &config, "https://x.com/a")
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output_text.as_deref(), Some("The body."));
    }

    #[tokio::test]
    async fn not_configured() {
        let config = KagiConfig {
            session_token: String::new(),
            language: None,
        };
        // No mock server needed — the not-configured early return fires before any HTTP call
        let result = summarize_url_with_base("http://unused.invalid", &config, "https://x.com/a")
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or("")
                .contains("not configured")
        );
    }

    #[tokio::test]
    async fn markdown_without_title_prefix() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "output_data": {"markdown": "Plain body"}
            })))
            .mount(&server)
            .await;
        let config = KagiConfig {
            session_token: "tok".into(),
            language: None,
        };
        let result = summarize_url_with_base(&server.uri(), &config, "https://x.com/a")
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output_text.as_deref(), Some("Plain body"));
    }

    #[tokio::test]
    async fn error_field_in_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "error": "nope"
            })))
            .mount(&server)
            .await;
        let config = KagiConfig {
            session_token: "tok".into(),
            language: None,
        };
        let result = summarize_url_with_base(&server.uri(), &config, "https://x.com/a")
            .await
            .unwrap();
        assert!(!result.success);
        assert_eq!(result.error.as_deref(), Some("nope"));
    }

    #[tokio::test]
    async fn no_output_data() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let config = KagiConfig {
            session_token: "tok".into(),
            language: None,
        };
        let result = summarize_url_with_base(&server.uri(), &config, "https://x.com/a")
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("No summary"));
    }

    #[tokio::test]
    async fn invalid_token_401() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let config = KagiConfig {
            session_token: "tok".into(),
            language: None,
        };
        let result = summarize_url_with_base(&server.uri(), &config, "https://x.com/a")
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or("")
                .contains("Invalid session token")
        );
    }

    #[tokio::test]
    async fn forbidden_403() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let config = KagiConfig {
            session_token: "tok".into(),
            language: None,
        };
        let result = summarize_url_with_base(&server.uri(), &config, "https://x.com/a")
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or("")
                .contains("Access forbidden")
        );
    }

    #[tokio::test]
    async fn rate_limited_429() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;
        let config = KagiConfig {
            session_token: "tok".into(),
            language: None,
        };
        let result = summarize_url_with_base(&server.uri(), &config, "https://x.com/a")
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("Rate limit"));
    }

    #[tokio::test]
    async fn server_error_500() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500).set_body_string("x"))
            .mount(&server)
            .await;
        let config = KagiConfig {
            session_token: "tok".into(),
            language: None,
        };
        let result = summarize_url_with_base(&server.uri(), &config, "https://x.com/a")
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or("")
                .contains("Kagi error (500")
        );
    }

    #[tokio::test]
    async fn malformed_json() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{bad"))
            .mount(&server)
            .await;
        let config = KagiConfig {
            session_token: "tok".into(),
            language: None,
        };
        let result = summarize_url_with_base(&server.uri(), &config, "https://x.com/a").await;
        assert!(matches!(result, Err(AppError::Internal(_))));
    }

    #[tokio::test]
    async fn summarize_url_honors_kagi_api_base_env() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "output_data": {"markdown": "E2E mock summary body."}
            })))
            .mount(&server)
            .await;
        let config = KagiConfig {
            session_token: "tok".into(),
            language: None,
        };
        // `set_var`/`remove_var` are `unsafe` in this toolchain (process-global
        // mutation); the `unsafe` blocks are required. cargo-nextest runs each
        // test in its own process, so this env mutation cannot race other tests.
        // Test-only env mutation; nextest isolates each test in its own process.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("RDRS_KAGI_API_BASE", server.uri());
        };
        let result = summarize_url(&config, "https://x.com/a").await.unwrap();
        // Test-only env mutation; nextest isolates each test in its own process.
        #[allow(unsafe_code)]
        unsafe {
            std::env::remove_var("RDRS_KAGI_API_BASE");
        };
        assert!(result.success);
        assert_eq!(
            result.output_text.as_deref(),
            Some("E2E mock summary body.")
        );
    }

    #[tokio::test]
    async fn language_adds_query_param() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("target_language", "fr"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "output_data": {"markdown": "Résumé"}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let config = KagiConfig {
            session_token: "tok".into(),
            language: Some("fr".into()),
        };
        let result = summarize_url_with_base(&server.uri(), &config, "https://x.com/a")
            .await
            .unwrap();
        assert!(result.success);
        // MockServer verifies the query param expectation on drop
    }

    #[tokio::test]
    async fn summarize_success_extracts_title() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "output_data": {"markdown": "Title: Foo Bar\n\nThe body."}
            })))
            .mount(&server)
            .await;
        let config = KagiConfig {
            session_token: "t".into(),
            language: None,
        };
        let result = summarize_url_with_base(&server.uri(), &config, "https://x.com/a")
            .await
            .unwrap();
        assert_eq!(result.title.as_deref(), Some("Foo Bar"));
        assert_eq!(result.output_text.as_deref(), Some("The body."));
    }

    #[tokio::test]
    async fn summarize_success_title_none_when_absent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "output_data": {"markdown": "Plain body, no title line."}
            })))
            .mount(&server)
            .await;
        let config = KagiConfig {
            session_token: "t".into(),
            language: None,
        };
        let result = summarize_url_with_base(&server.uri(), &config, "https://x.com/a")
            .await
            .unwrap();
        assert_eq!(result.title, None);
        assert_eq!(
            result.output_text.as_deref(),
            Some("Plain body, no title line.")
        );
    }
}
