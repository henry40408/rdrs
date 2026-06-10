use readability::extractor;
use reqwest::header::USER_AGENT;
use url::Url;

use crate::error::{AppError, AppResult};
use crate::services::http::{send_with_retry_on_error, RetryConfig, SHARED_CLIENT};
use crate::utils::url_validation;

pub struct ExtractedContent {
    pub title: Option<String>,
    pub content: String,
}

/// Fetches HTML from URL and extracts readable content using readability crate.
pub async fn fetch_and_extract(url: &str, user_agent: &str) -> AppResult<ExtractedContent> {
    // Parse and validate URL (SSRF protection)
    let parsed_url = Url::parse(url).map_err(|_| AppError::InvalidUrl)?;
    url_validation::validate_url(&parsed_url).map_err(|_| AppError::InvalidUrl)?;

    // Fetch HTML via the shared, connection-pooled client (User-Agent per request).
    let url_owned = url.to_string();
    let response = send_with_retry_on_error(&RetryConfig::default(), || {
        SHARED_CLIENT.get(&url_owned).header(USER_AGENT, user_agent)
    })
    .await
    .map_err(|e| AppError::FetchError(e.to_string()))?;

    if !response.status().is_success() {
        return Err(AppError::FetchError(format!("HTTP {}", response.status())));
    }

    let html = response
        .text()
        .await
        .map_err(|e| AppError::FetchError(e.to_string()))?;

    // Extract readable content
    let product = extractor::extract(&mut html.as_bytes(), &parsed_url)
        .map_err(|e| AppError::FetchError(format!("Failed to extract content: {}", e)))?;

    Ok(ExtractedContent {
        title: Some(product.title).filter(|t| !t.is_empty()),
        content: product.content,
    })
}
