use readability::extractor;
use reqwest::header::USER_AGENT;
use url::Url;

use crate::error::{AppError, AppResult};
use crate::services::http::{RetryConfig, SHARED_CLIENT, send_with_retry_on_error};
use crate::utils::url_validation;

#[derive(Debug)]
pub struct ExtractedContent {
    pub title: Option<String>,
    pub content: String,
}

/// Fetches HTML from URL and extracts readable content using readability crate.
pub async fn fetch_and_extract(url: &str, user_agent: &str) -> AppResult<ExtractedContent> {
    // Parse and validate URL (SSRF protection) before touching the network.
    let parsed_url = Url::parse(url).map_err(|_e| AppError::InvalidUrl)?;
    url_validation::validate_url(&parsed_url).map_err(|_e| AppError::InvalidUrl)?;

    fetch_and_extract_validated(url, &parsed_url, user_agent).await
}

/// Fetch + extract for an already-validated URL. Split out from
/// [`fetch_and_extract`] so the network/extraction path can be exercised against
/// a local mock server (the SSRF check in the public entry rejects loopback).
async fn fetch_and_extract_validated(
    url: &str,
    parsed_url: &Url,
    user_agent: &str,
) -> AppResult<ExtractedContent> {
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
    let product = extractor::extract(&mut html.as_bytes(), parsed_url)
        .map_err(|e| AppError::FetchError(format!("Failed to extract content: {}", e)))?;

    Ok(ExtractedContent {
        title: Some(product.title).filter(|t| !t.is_empty()),
        content: product.content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ARTICLE_HTML: &str = r#"<!DOCTYPE html>
<html>
  <head><title>The Readable Title</title></head>
  <body>
    <nav>skip me</nav>
    <article>
      <h1>The Readable Title</h1>
      <p>This is the first substantial paragraph of the article body that
      readability should retain because it carries the real content.</p>
      <p>A second paragraph with even more meaningful prose so the extractor
      is confident this is the main article region and not boilerplate.</p>
    </article>
    <footer>copyright boilerplate</footer>
  </body>
</html>"#;

    // ---- Public entry: SSRF validation happens before any network I/O. ----

    #[tokio::test]
    async fn rejects_malformed_url() {
        let err = fetch_and_extract("not a url", "RDRS-Test/1.0")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidUrl));
    }

    #[tokio::test]
    async fn rejects_blocked_loopback_host() {
        let err = fetch_and_extract("http://localhost/article", "RDRS-Test/1.0")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidUrl));
    }

    #[tokio::test]
    async fn rejects_non_http_scheme() {
        let err = fetch_and_extract("ftp://example.com/article", "RDRS-Test/1.0")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidUrl));
    }

    // ---- Fetch + extract path, driven against a local mock server. ----

    #[tokio::test]
    async fn extracts_title_and_content_and_sends_user_agent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/article"))
            .and(header(USER_AGENT.as_str(), "RDRS-Test/1.0"))
            .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
            .expect(1)
            .mount(&server)
            .await;

        let url = format!("{}/article", server.uri());
        let parsed = Url::parse(&url).unwrap();
        let extracted = fetch_and_extract_validated(&url, &parsed, "RDRS-Test/1.0")
            .await
            .unwrap();

        assert_eq!(extracted.title.as_deref(), Some("The Readable Title"));
        assert!(
            extracted.content.contains("first substantial paragraph"),
            "content should retain the article body, got: {}",
            extracted.content
        );
        // MockServer verifies .expect(1) on drop — proves the User-Agent matched.
    }

    #[tokio::test]
    async fn empty_extracted_title_becomes_none() {
        // No <title> and no heading: readability yields an empty title string,
        // which the `.filter(|t| !t.is_empty())` must collapse to None.
        let body = r#"<!DOCTYPE html><html><head></head><body><article>
            <p>Body-only content with no title anywhere in the document, long
            enough that the extractor keeps it as the main article region.</p>
            <p>Another paragraph of prose to reinforce the content region.</p>
            </article></body></html>"#;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;

        let url = server.uri();
        let parsed = Url::parse(&url).unwrap();
        let extracted = fetch_and_extract_validated(&url, &parsed, "RDRS-Test/1.0")
            .await
            .unwrap();

        assert!(
            extracted.title.is_none(),
            "empty title should become None, got: {:?}",
            extracted.title
        );
    }

    #[tokio::test]
    async fn non_success_status_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let url = format!("{}/missing", server.uri());
        let parsed = Url::parse(&url).unwrap();
        let err = fetch_and_extract_validated(&url, &parsed, "RDRS-Test/1.0")
            .await
            .unwrap_err();

        match err {
            AppError::FetchError(msg) => assert!(msg.contains("404"), "unexpected message: {msg}"),
            other => panic!("expected FetchError, got {other:?}"),
        }
    }
}
