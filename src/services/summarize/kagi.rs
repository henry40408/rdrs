use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

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

/// Response from Kagi Summary API
#[derive(Debug, Deserialize)]
struct KagiSummaryResponse {
    output_text: Option<String>,
    error: Option<String>,
}

/// Result of summarization
#[derive(Debug, Clone, Serialize)]
pub struct SummarizeResult {
    pub success: bool,
    pub output_text: Option<String>,
    pub error: Option<String>,
}

/// Summarize a URL using Kagi Universal Summarizer
pub async fn summarize_url(config: &KagiConfig, url: &str) -> AppResult<SummarizeResult> {
    if !config.is_configured() {
        return Ok(SummarizeResult {
            success: false,
            output_text: None,
            error: Some("Kagi is not configured".to_string()),
        });
    }

    let client = Client::new();

    // Build the API URL with query parameters
    let mut api_url = url::Url::parse("https://kagi.com/mother/summary_labs")
        .map_err(|e| AppError::Internal(format!("Failed to parse Kagi API URL: {}", e)))?;

    {
        let mut query = api_url.query_pairs_mut();
        query.append_pair("summary_type", "summary");
        query.append_pair("url", url);

        if let Some(lang) = &config.language {
            if !lang.is_empty() {
                query.append_pair("target_language", lang);
            }
        }
    }

    let response = client
        .get(api_url)
        .header("Authorization", &config.session_token)
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to connect to Kagi: {}", e)))?;

    let status = response.status();

    if status.is_success() {
        let body: KagiSummaryResponse = response
            .json()
            .await
            .map_err(|e| AppError::Internal(format!("Failed to parse Kagi response: {}", e)))?;

        if let Some(error) = body.error {
            Ok(SummarizeResult {
                success: false,
                output_text: None,
                error: Some(error),
            })
        } else if let Some(output) = body.output_text {
            Ok(SummarizeResult {
                success: true,
                output_text: Some(output),
                error: None,
            })
        } else {
            Ok(SummarizeResult {
                success: false,
                output_text: None,
                error: Some("No summary returned from Kagi".to_string()),
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
            _ => format!("Kagi error ({}): {}", status, error_text),
        };

        Ok(SummarizeResult {
            success: false,
            output_text: None,
            error: Some(message),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kagi_config_is_configured() {
        let config = KagiConfig {
            session_token: "some_token".to_string(),
            language: Some("ZH-HANT".to_string()),
        };
        assert!(config.is_configured());

        let empty_token = KagiConfig {
            session_token: "".to_string(),
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
}
