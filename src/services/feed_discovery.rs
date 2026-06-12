use reqwest::header::USER_AGENT;
use scraper::{Html, Selector};
use url::Url;

use crate::error::{AppError, AppResult};
use crate::services::http::{send_with_retry_on_error, RetryConfig, SHARED_CLIENT};

#[derive(Debug, Clone)]
pub struct DiscoveredFeed {
    pub feed_url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_url: Option<String>,
}

pub async fn discover_feed(url: &str, user_agent: &str) -> AppResult<DiscoveredFeed> {
    // Validate URL
    let parsed_url = Url::parse(url).map_err(|_| AppError::InvalidUrl)?;

    if parsed_url.scheme() != "http" && parsed_url.scheme() != "https" {
        return Err(AppError::InvalidUrl);
    }

    // Fetch the URL via the shared, connection-pooled client (UA per request).
    let retry_config = RetryConfig::default();
    let url_owned = url.to_string();

    let response = send_with_retry_on_error(&retry_config, || {
        SHARED_CLIENT.get(&url_owned).header(USER_AGENT, user_agent)
    })
    .await
    .map_err(|e| AppError::FetchError(e.to_string()))?;

    if !response.status().is_success() {
        return Err(AppError::FetchError(format!("HTTP {}", response.status())));
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|ct| ct.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let body = response
        .text()
        .await
        .map_err(|e| AppError::FetchError(e.to_string()))?;

    // Check if this is a feed
    if is_feed_content_type(&content_type) || looks_like_feed(&body) {
        return parse_feed_content(url, &body);
    }

    // It's HTML, try to find feed links
    let feed_url = find_feed_link_in_html(&body, &parsed_url)?;

    // Fetch and parse the discovered feed
    let feed_response = send_with_retry_on_error(&retry_config, || {
        SHARED_CLIENT.get(&feed_url).header(USER_AGENT, user_agent)
    })
    .await
    .map_err(|e| AppError::FetchError(e.to_string()))?;

    if !feed_response.status().is_success() {
        return Err(AppError::FetchError(format!(
            "HTTP {}",
            feed_response.status()
        )));
    }

    let feed_body = feed_response
        .text()
        .await
        .map_err(|e| AppError::FetchError(e.to_string()))?;

    parse_feed_content(&feed_url, &feed_body)
}

fn is_feed_content_type(content_type: &str) -> bool {
    content_type.contains("application/rss")
        || content_type.contains("application/atom")
        || content_type.contains("application/xml")
        || content_type.contains("text/xml")
}

fn looks_like_feed(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with("<?xml")
        || trimmed.starts_with("<rss")
        || trimmed.starts_with("<feed")
        || trimmed.starts_with("<RDF")
}

fn find_feed_link_in_html(html: &str, base_url: &Url) -> AppResult<String> {
    let document = Html::parse_document(html);

    let selector = Selector::parse(
        r#"link[rel="alternate"][type="application/rss+xml"],
           link[rel="alternate"][type="application/atom+xml"],
           link[rel="alternate"][type="application/xml"]"#,
    )
    .map_err(|_| AppError::Internal("Failed to parse selector".to_string()))?;

    for element in document.select(&selector) {
        if let Some(href) = element.value().attr("href") {
            let feed_url = base_url
                .join(href)
                .map_err(|_| AppError::InvalidUrl)?
                .to_string();
            return Ok(feed_url);
        }
    }

    Err(AppError::NoFeedFound)
}

fn parse_feed_content(feed_url: &str, content: &str) -> AppResult<DiscoveredFeed> {
    let feed = feed_rs::parser::parse(content.as_bytes())
        .map_err(|e| AppError::FeedParseError(e.to_string()))?;

    let title = feed.title.map(|t| t.content);

    let description = feed.description.map(|d| d.content);

    let site_url = feed
        .links
        .iter()
        .find(|link| link.rel.as_deref() == Some("alternate") || link.rel.is_none())
        .map(|link| link.href.clone());

    Ok(DiscoveredFeed {
        feed_url: feed_url.to_string(),
        title,
        description,
        site_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const RSS: &str = r#"<?xml version="1.0"?><rss version="2.0"><channel>
        <title>Example Feed</title><description>Desc</description>
        <link>https://example.com</link></channel></rss>"#;

    #[tokio::test]
    async fn discover_direct_feed_by_content_type() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(RSS),
            )
            .mount(&server)
            .await;
        let result = discover_feed(&server.uri(), "RDRS-Test/1.0").await.unwrap();
        assert_eq!(result.title.as_deref(), Some("Example Feed"));
        assert_eq!(result.description.as_deref(), Some("Desc"));
    }

    #[tokio::test]
    async fn discover_via_html_link() {
        let server = MockServer::start().await;
        let html = r#"<html><head>
            <link rel="alternate" type="application/rss+xml" href="/feed.xml">
            </head><body>hi</body></html>"#;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string(html),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/feed.xml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(RSS),
            )
            .mount(&server)
            .await;
        let result = discover_feed(&format!("{}/", server.uri()), "RDRS-Test/1.0")
            .await
            .unwrap();
        assert_eq!(result.feed_url, format!("{}/feed.xml", server.uri()));
        assert_eq!(result.title.as_deref(), Some("Example Feed"));
    }

    #[tokio::test]
    async fn rejects_invalid_url() {
        let result = discover_feed("not a url", "ua").await;
        assert!(matches!(result, Err(AppError::InvalidUrl)));
    }

    #[tokio::test]
    async fn rejects_non_http_scheme() {
        let result = discover_feed("ftp://example.com", "ua").await;
        assert!(matches!(result, Err(AppError::InvalidUrl)));
    }

    #[tokio::test]
    async fn discover_by_body_sniffing() {
        // content-type is text/html but body looks like a feed — covers looks_like_feed
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string(RSS),
            )
            .mount(&server)
            .await;
        let result = discover_feed(&server.uri(), "RDRS-Test/1.0").await.unwrap();
        assert_eq!(result.title.as_deref(), Some("Example Feed"));
    }

    #[tokio::test]
    async fn html_without_feed_link_errors() {
        let server = MockServer::start().await;
        let html = "<html><head></head><body>no feed here</body></html>";
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string(html),
            )
            .mount(&server)
            .await;
        let result = discover_feed(&format!("{}/", server.uri()), "RDRS-Test/1.0").await;
        assert!(matches!(result, Err(AppError::NoFeedFound)));
    }

    #[tokio::test]
    async fn first_fetch_non_success_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let result = discover_feed(&server.uri(), "RDRS-Test/1.0").await;
        assert!(matches!(result, Err(AppError::FetchError(_))));
    }

    #[tokio::test]
    async fn discovered_fetch_non_success_errors() {
        let server = MockServer::start().await;
        let html = r#"<html><head>
            <link rel="alternate" type="application/rss+xml" href="/feed.xml">
            </head><body>hi</body></html>"#;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string(html),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/feed.xml"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let result = discover_feed(&format!("{}/", server.uri()), "RDRS-Test/1.0").await;
        assert!(matches!(result, Err(AppError::FetchError(_))));
    }

    #[tokio::test]
    async fn malformed_feed_body_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string("<rss><broken"),
            )
            .mount(&server)
            .await;
        let result = discover_feed(&server.uri(), "RDRS-Test/1.0").await;
        assert!(matches!(result, Err(AppError::FeedParseError(_))));
    }

    #[tokio::test]
    async fn sends_user_agent_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("user-agent", "RDRS-Test/1.0"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(RSS),
            )
            .expect(1)
            .mount(&server)
            .await;
        discover_feed(&server.uri(), "RDRS-Test/1.0").await.unwrap();
        // MockServer verifies .expect(1) on drop
    }

    #[test]
    fn is_feed_content_type_unit() {
        assert!(is_feed_content_type("application/rss+xml"));
        assert!(is_feed_content_type("application/atom+xml"));
        assert!(is_feed_content_type("application/xml"));
        assert!(is_feed_content_type("text/xml"));
        assert!(!is_feed_content_type("text/html"));
    }

    #[test]
    fn looks_like_feed_unit() {
        assert!(looks_like_feed("<?xml version"));
        assert!(looks_like_feed("<rss version="));
        assert!(looks_like_feed("<feed xmlns="));
        assert!(looks_like_feed("<RDF:RDF"));
        assert!(!looks_like_feed("random text"));
        assert!(!looks_like_feed("<html>"));
    }

    #[test]
    fn find_feed_link_relative_resolution() {
        let html = r#"<html><head>
            <link rel="alternate" type="application/rss+xml" href="/f.xml">
            </head></html>"#;
        let base = Url::parse("https://x.com/a/b").unwrap();
        let result = find_feed_link_in_html(html, &base).unwrap();
        assert_eq!(result, "https://x.com/f.xml");
    }
}
