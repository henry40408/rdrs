use dom_smoothie::{Config, Readability};
use reqwest::header::USER_AGENT;
use url::Url;

use crate::error::{AppError, AppResult};
use crate::services::fetch::Fetcher;
use crate::services::http::{RetryConfig, send_with_retry_on_error};

/// Upper bound on element nodes in a fetched document; `dom_smoothie` rejects
/// larger documents up front (`ReadabilityError::TooManyElements`) before any
/// scoring work, bounding CPU and memory on hostile or pathological pages.
///
/// Measured element counts (`dom_smoothie`'s own count, 2026-10): HN front page
/// 818, MDN `Array` reference 2,176, Wikipedia "List of countries by GDP"
/// 5,556, "World War II" 16,890, "United States" 21,837, "List of Latin
/// phrases (full)" 27,588. 100k leaves ~3.6x headroom over the largest of
/// those while still capping runaway documents.
const MAX_ELEMENTS_TO_PARSE: usize = 100_000;

#[derive(Debug)]
pub struct ExtractedContent {
    pub title: Option<String>,
    pub content: String,
}

/// Fetch HTML from a URL and extract readable content.
pub async fn fetch_and_extract(
    url: &str,
    user_agent: &str,
    fetcher: &Fetcher,
) -> AppResult<ExtractedContent> {
    // SSRF check before touching the network; the fetcher re-checks every hop
    // and resolved address.
    let parsed_url = Url::parse(url).map_err(|_e| AppError::InvalidUrl)?;
    fetcher
        .validate(&parsed_url)
        .map_err(|_e| AppError::InvalidUrl)?;

    fetch_and_extract_validated(url, &parsed_url, user_agent, fetcher).await
}

/// Fetch + extract for an already-validated URL, split out so tests can hit a
/// loopback mock server.
async fn fetch_and_extract_validated(
    url: &str,
    parsed_url: &Url,
    user_agent: &str,
    fetcher: &Fetcher,
) -> AppResult<ExtractedContent> {
    let url_owned = url.to_string();
    let response = send_with_retry_on_error(&RetryConfig::default(), || {
        fetcher
            .client(false)
            .get(&url_owned)
            .header(USER_AGENT, user_agent)
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

    // `Readability` holds a non-`Send` DOM, so build and drop it entirely
    // inside the blocking task; only owned strings cross the boundary.
    let base_url = parsed_url.to_string();
    let article = tokio::task::spawn_blocking(move || {
        extract_article(html, &base_url, MAX_ELEMENTS_TO_PARSE)
    })
    .await
    .map_err(|e| AppError::FetchError(format!("Failed to extract content: {e}")))?
    .map_err(|e| AppError::FetchError(format!("Failed to extract content: {e}")))?;

    let (title, content) = article;
    Ok(ExtractedContent {
        title: Some(title).filter(|t| !t.is_empty()),
        content,
    })
}

/// Parse `html` into `(title, content)`, rejecting documents with more than
/// `max_elements` element nodes.
fn extract_article(
    html: String,
    base_url: &str,
    max_elements: usize,
) -> Result<(String, String), String> {
    let config = Config {
        max_elements_to_parse: max_elements,
        ..Config::default()
    };
    let mut readability =
        Readability::new(html, Some(base_url), Some(config)).map_err(|e| e.to_string())?;
    let article = readability.parse().map_err(|e| e.to_string())?;
    Ok((article.title, article.content.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::url_validation::FetchPolicy;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// wiremock binds loopback, so allow it the same way a deployment opts in a
    /// LAN feed.
    fn loopback_fetcher() -> Fetcher {
        Fetcher::new(FetchPolicy::parse("127.0.0.1").expect("valid allow list"))
            .expect("the guarded client must build")
    }

    const ARTICLE_HTML: &str = r"<!DOCTYPE html>
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
</html>";

    // ---- Public entry: SSRF validation before any network I/O. ----

    #[tokio::test]
    async fn rejects_malformed_blocked_and_non_http_urls() {
        for url in [
            "not a url",
            "http://localhost/article",
            "ftp://example.com/article",
        ] {
            let err = fetch_and_extract(url, "RDRS-Test/1.0", &Fetcher::default())
                .await
                .unwrap_err();
            assert!(matches!(err, AppError::InvalidUrl), "{url}: {err:?}");
        }
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
        let extracted =
            fetch_and_extract_validated(&url, &parsed, "RDRS-Test/1.0", &loopback_fetcher())
                .await
                .unwrap();

        assert_eq!(extracted.title.as_deref(), Some("The Readable Title"));
        assert!(
            extracted.content.contains("first substantial paragraph"),
            "content should retain the article body, got: {}",
            extracted.content
        );
        // `.expect(1)` proves the User-Agent matched.
    }

    #[tokio::test]
    async fn empty_extracted_title_becomes_none() {
        // No title or heading: the empty string must collapse to None.
        let body = r"<!DOCTYPE html><html><head></head><body><article>
            <p>Body-only content with no title anywhere in the document, long
            enough that the extractor keeps it as the main article region.</p>
            <p>Another paragraph of prose to reinforce the content region.</p>
            </article></body></html>";
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;

        let url = server.uri();
        let parsed = Url::parse(&url).unwrap();
        let extracted =
            fetch_and_extract_validated(&url, &parsed, "RDRS-Test/1.0", &loopback_fetcher())
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
        let err = fetch_and_extract_validated(&url, &parsed, "RDRS-Test/1.0", &loopback_fetcher())
            .await
            .unwrap_err();

        match err {
            AppError::FetchError(msg) => assert!(msg.contains("404"), "unexpected message: {msg}"),
            other => panic!("expected FetchError, got {other:?}"),
        }
    }

    // ---- Element-count limit. ----

    #[test]
    fn extract_article_rejects_documents_over_the_element_limit() {
        // 100 spans plus html/head/body/article/p is well over a limit of 50.
        let spans = "<span>x</span>".repeat(100);
        let html =
            format!("<html><head></head><body><article><p>{spans}</p></article></body></html>");

        let err = extract_article(html.clone(), "https://example.com/", 50).unwrap_err();
        assert!(err.contains("too many elements"), "unexpected: {err}");

        // Same document passes when the limit allows it.
        extract_article(html, "https://example.com/", 1_000).unwrap();
    }

    #[test]
    fn article_fixture_is_well_under_the_production_limit() {
        let (title, content) = extract_article(
            ARTICLE_HTML.to_string(),
            "https://example.com/",
            MAX_ELEMENTS_TO_PARSE,
        )
        .unwrap();
        assert_eq!(title, "The Readable Title");
        assert!(content.contains("first substantial paragraph"));
    }

    #[tokio::test]
    async fn oversized_document_maps_to_fetch_error() {
        let spans = "<i></i>".repeat(MAX_ELEMENTS_TO_PARSE + 1);
        let body =
            format!("<html><head></head><body><article><p>{spans}</p></article></body></html>");
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;

        let url = server.uri();
        let parsed = Url::parse(&url).unwrap();
        let err = fetch_and_extract_validated(&url, &parsed, "RDRS-Test/1.0", &loopback_fetcher())
            .await
            .unwrap_err();

        match err {
            AppError::FetchError(msg) => {
                assert!(msg.contains("Failed to extract content"), "{msg}");
                assert!(msg.contains("too many elements"), "{msg}");
            }
            other => panic!("expected FetchError, got {other:?}"),
        }
    }
}
