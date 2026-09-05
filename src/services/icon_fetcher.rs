use reqwest::header::{CONTENT_TYPE, USER_AGENT};
use tracing::debug;
use url::Url;

use crate::error::AppResult;
use crate::services::http::{ICON_TIMEOUT, RetryConfig, SHARED_CLIENT, send_with_retry_on_error};
use crate::utils::url_validation::FetchPolicy;

const MAX_ICON_SIZE: usize = 256 * 1024; // 256KB

pub struct FetchedImage {
    pub data: Vec<u8>,
    pub content_type: String,
    pub source_url: String,
}

pub async fn fetch_feed_icon(
    icon_url: Option<&str>,
    logo_url: Option<&str>,
    site_url: Option<&str>,
    user_agent: &str,
    policy: &FetchPolicy,
) -> AppResult<Option<FetchedImage>> {
    if let Some(url) = icon_url
        && let Ok(Some(img)) = fetch_image(url, user_agent, policy).await
    {
        debug!(
            event = "icon.fetched",
            source = "icon_url",
            url,
            "fetched feed icon"
        );
        return Ok(Some(img));
    }

    if let Some(url) = logo_url
        && let Ok(Some(img)) = fetch_image(url, user_agent, policy).await
    {
        debug!(
            event = "icon.fetched",
            source = "logo_url",
            url,
            "fetched feed icon"
        );
        return Ok(Some(img));
    }

    // Fallback to favicon
    if let Some(url) = site_url
        && let Ok(Some(img)) = fetch_favicon(url, user_agent, policy).await
    {
        return Ok(Some(img));
    }

    Ok(None)
}

async fn fetch_image(
    url: &str,
    user_agent: &str,
    policy: &FetchPolicy,
) -> AppResult<Option<FetchedImage>> {
    // Every icon URL here is attacker-controlled in the ordinary case: `<icon>`,
    // `<logo>` and the favicon `<link>` all come out of a document the feed
    // publisher wrote. A rejected URL is not an error the user needs to see —
    // a missing icon is cosmetic — so it logs and yields `None` like every
    // other rejection in this function.
    let Ok(parsed) = Url::parse(url) else {
        debug!(
            event = "icon.rejected",
            reason = "unparsable",
            url,
            "image URL is not a valid URL"
        );
        return Ok(None);
    };
    if let Err(e) = policy.validate(&parsed) {
        debug!(event = "icon.rejected", reason = "blocked_url", url, error = %e, "image URL points somewhere private");
        return Ok(None);
    }

    let retry_config = RetryConfig::icon();
    let url_owned = url.to_string();

    let response = match send_with_retry_on_error(&retry_config, || {
        SHARED_CLIENT
            .get(&url_owned)
            .timeout(ICON_TIMEOUT)
            .header(USER_AGENT, user_agent)
    })
    .await
    {
        Ok(r) => r,
        Err(e) => {
            debug!(event = "icon.fetch_failed", url, error = %e, "failed to fetch image");
            return Ok(None);
        }
    };

    if !response.status().is_success() {
        debug!(event = "icon.rejected", reason = "status", status = %response.status(), url, "image request returned a non-success status");
        return Ok(None);
    }

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
        .unwrap_or_default();

    if !content_type.starts_with("image/") {
        debug!(
            event = "icon.rejected",
            reason = "content_type",
            content_type,
            url,
            "image has a non-image content type"
        );
        return Ok(None);
    }

    let bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => {
            debug!(event = "icon.fetch_failed", url, error = %e, "failed to read image body");
            return Ok(None);
        }
    };

    if bytes.len() > MAX_ICON_SIZE {
        debug!(
            event = "icon.rejected",
            reason = "too_large",
            bytes = bytes.len(),
            url,
            "image exceeds the size limit"
        );
        return Ok(None);
    }

    if bytes.is_empty() {
        debug!(
            event = "icon.rejected",
            reason = "empty",
            url,
            "image body is empty"
        );
        return Ok(None);
    }

    Ok(Some(FetchedImage {
        data: bytes.to_vec(),
        content_type,
        source_url: url.to_string(),
    }))
}

async fn fetch_favicon(
    site_url: &str,
    user_agent: &str,
    policy: &FetchPolicy,
) -> AppResult<Option<FetchedImage>> {
    let Ok(base_url) = Url::parse(site_url) else {
        return Ok(None);
    };
    if let Err(e) = policy.validate(&base_url) {
        debug!(event = "icon.rejected", reason = "blocked_url", url = site_url, error = %e, "site URL points somewhere private");
        return Ok(None);
    }

    let Ok(favicon_url) = base_url.join("/favicon.ico") else {
        return Ok(None);
    };
    if let Ok(Some(img)) = fetch_image(favicon_url.as_str(), user_agent, policy).await {
        debug!(event = "icon.fetched", source = "favicon_ico", url = %favicon_url, "fetched favicon");
        return Ok(Some(img));
    }

    let retry_config = RetryConfig::icon();
    let site_url_owned = site_url.to_string();

    let html = match send_with_retry_on_error(&retry_config, || {
        SHARED_CLIENT
            .get(&site_url_owned)
            .timeout(ICON_TIMEOUT)
            .header(USER_AGENT, user_agent)
    })
    .await
    {
        Ok(r) if r.status().is_success() => match r.text().await {
            Ok(t) => t,
            Err(_) => return Ok(None),
        },
        _ => return Ok(None),
    };

    if let Some(icon_url) = extract_favicon_from_html(&html, &base_url)
        && let Ok(Some(img)) = fetch_image(&icon_url, user_agent, policy).await
    {
        debug!(
            event = "icon.fetched",
            source = "html_link",
            url = icon_url,
            "fetched favicon"
        );
        return Ok(Some(img));
    }

    Ok(None)
}

fn extract_favicon_from_html(html: &str, base_url: &Url) -> Option<String> {
    let html_lower = html.to_lowercase();

    // Look for <link rel="icon" or <link rel="shortcut icon"
    for pattern in &[
        "rel=\"icon\"",
        "rel='icon'",
        "rel=\"shortcut icon\"",
        "rel='shortcut icon'",
    ] {
        if let Some(link_pos) = html_lower.find(pattern) {
            // Find the start of this <link> tag
            let tag_start = html_lower[..link_pos].rfind("<link")?;
            // Find the end of this tag
            let tag_end = html_lower[tag_start..].find('>')? + tag_start;
            let tag = &html[tag_start..=tag_end];

            // Extract href
            if let Some(href) = extract_href(tag) {
                return resolve_url(&href, base_url);
            }
        }
    }

    None
}

fn extract_href(tag: &str) -> Option<String> {
    let tag_lower = tag.to_lowercase();

    for prefix in &["href=\"", "href='"] {
        if let Some(start) = tag_lower.find(prefix) {
            let quote = if prefix.ends_with('"') { '"' } else { '\'' };
            let value_start = start + prefix.len();
            let value_end = tag[value_start..].find(quote)?;
            return Some(tag[value_start..value_start + value_end].to_string());
        }
    }

    None
}

fn resolve_url(href: &str, base_url: &Url) -> Option<String> {
    if href.starts_with("http://") || href.starts_with("https://") {
        return Some(href.to_string());
    }

    if href.starts_with("//") {
        return Some(format!("{}:{}", base_url.scheme(), href));
    }

    base_url.join(href).ok().map(|u| u.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// The suites here point every fetcher at a `wiremock` server, which binds
    /// loopback — exactly what the SSRF guard exists to refuse. Allowing that
    /// one address is what a deployment does for a LAN feed, so the tests take
    /// the same route rather than a bypass of their own.
    fn loopback_policy() -> FetchPolicy {
        FetchPolicy::parse("127.0.0.1").expect("valid allow list")
    }

    const PNG_BYTES: &[u8] = &[0x89, 0x50, 0x4E, 0x47]; // PNG magic; non-empty

    #[tokio::test]
    async fn fetch_image_success_returns_bytes_and_type() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(PNG_BYTES.to_vec()),
            )
            .mount(&server)
            .await;
        let img = fetch_image(&server.uri(), "RDRS-Test/1.0", &loopback_policy())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(img.content_type, "image/png");
        assert_eq!(img.data, PNG_BYTES);
        assert_eq!(img.source_url, server.uri());
    }

    #[tokio::test]
    async fn fetch_image_strips_content_type_params() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/svg+xml; charset=utf-8")
                    .set_body_bytes(PNG_BYTES.to_vec()),
            )
            .mount(&server)
            .await;
        let img = fetch_image(&server.uri(), "RDRS-Test/1.0", &loopback_policy())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(img.content_type, "image/svg+xml");
    }

    #[tokio::test]
    async fn fetch_image_non_success_is_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let result = fetch_image(&server.uri(), "RDRS-Test/1.0", &loopback_policy())
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn fetch_image_non_image_type_is_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_bytes(b"hi".to_vec()),
            )
            .mount(&server)
            .await;
        let result = fetch_image(&server.uri(), "RDRS-Test/1.0", &loopback_policy())
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn fetch_image_missing_type_is_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(PNG_BYTES.to_vec()))
            .mount(&server)
            .await;
        let result = fetch_image(&server.uri(), "RDRS-Test/1.0", &loopback_policy())
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn fetch_image_too_large_is_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(vec![0u8; 300 * 1024]),
            )
            .mount(&server)
            .await;
        let result = fetch_image(&server.uri(), "RDRS-Test/1.0", &loopback_policy())
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn fetch_image_empty_body_is_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(vec![]),
            )
            .mount(&server)
            .await;
        let result = fetch_image(&server.uri(), "RDRS-Test/1.0", &loopback_policy())
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn fetch_favicon_uses_favicon_ico() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/favicon.ico"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/x-icon")
                    .set_body_bytes(PNG_BYTES.to_vec()),
            )
            .mount(&server)
            .await;
        let img = fetch_favicon(&server.uri(), "RDRS-Test/1.0", &loopback_policy())
            .await
            .unwrap()
            .unwrap();
        assert!(
            img.source_url.ends_with("/favicon.ico"),
            "source_url was: {}",
            img.source_url
        );
    }

    #[tokio::test]
    async fn fetch_favicon_falls_back_to_html_link() {
        let server = MockServer::start().await;

        // /favicon.ico → 404 so the fallback HTML-link path is exercised
        Mock::given(method("GET"))
            .and(path("/favicon.ico"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let icon_html =
            r#"<html><head><link rel="icon" href="/icon.png"></head></html>"#.to_string();
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string(icon_html),
            )
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/icon.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(PNG_BYTES.to_vec()),
            )
            .mount(&server)
            .await;

        let result = fetch_favicon(&server.uri(), "RDRS-Test/1.0", &loopback_policy())
            .await
            .unwrap();
        assert!(result.is_some());
        let img = result.unwrap();
        assert!(
            img.source_url.ends_with("/icon.png"),
            "source_url was: {}",
            img.source_url
        );
    }

    #[tokio::test]
    async fn fetch_favicon_no_icon_is_none() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/favicon.ico"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let html_no_icon = r"<html><head><title>No icon here</title></head></html>";
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string(html_no_icon),
            )
            .mount(&server)
            .await;

        let result = fetch_favicon(&server.uri(), "RDRS-Test/1.0", &loopback_policy())
            .await
            .unwrap();
        assert!(result.is_none());
    }

    /// `<icon>`, `<logo>` and the favicon `<link>` are all written by whoever
    /// publishes the feed, so an icon URL is a way to make the server fetch an
    /// address of the publisher's choosing every icon-refresh cycle.
    #[tokio::test]
    async fn fetch_feed_icon_refuses_urls_the_policy_does_not_allow() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(PNG_BYTES.to_vec()),
            )
            .expect(0)
            .mount(&server)
            .await;

        let icon_url = format!("{}/a", server.uri());
        let result = fetch_feed_icon(
            Some(&icon_url),
            Some("http://169.254.169.254/logo.png"),
            Some(&server.uri()),
            "RDRS-Test/1.0",
            &FetchPolicy::default(),
        )
        .await
        .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn fetch_feed_icon_prefers_icon_url() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/a"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(PNG_BYTES.to_vec()),
            )
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/b"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(PNG_BYTES.to_vec()),
            )
            .mount(&server)
            .await;

        let icon_url = format!("{}/a", server.uri());
        let logo_url = format!("{}/b", server.uri());
        let result = fetch_feed_icon(
            Some(&icon_url),
            Some(&logo_url),
            None,
            "RDRS-Test/1.0",
            &loopback_policy(),
        )
        .await
        .unwrap();
        assert!(result.is_some());
        let img = result.unwrap();
        assert!(
            img.source_url.ends_with("/a"),
            "source_url was: {}",
            img.source_url
        );
    }

    #[tokio::test]
    async fn fetch_feed_icon_falls_back_to_logo() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/a"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/b"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(PNG_BYTES.to_vec()),
            )
            .mount(&server)
            .await;

        let icon_url = format!("{}/a", server.uri());
        let logo_url = format!("{}/b", server.uri());
        let result = fetch_feed_icon(
            Some(&icon_url),
            Some(&logo_url),
            None,
            "RDRS-Test/1.0",
            &loopback_policy(),
        )
        .await
        .unwrap();
        assert!(result.is_some());
        let img = result.unwrap();
        assert!(
            img.source_url.ends_with("/b"),
            "source_url was: {}",
            img.source_url
        );
    }

    #[tokio::test]
    async fn fetch_feed_icon_all_none() {
        let result = fetch_feed_icon(None, None, None, "RDRS-Test/1.0", &loopback_policy())
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_href() {
        assert_eq!(
            extract_href(r#"<link rel="icon" href="/favicon.ico">"#),
            Some("/favicon.ico".to_string())
        );
        assert_eq!(
            extract_href(r"<link href='/icon.png' rel='icon'>"),
            Some("/icon.png".to_string())
        );
    }

    #[test]
    fn test_resolve_url() {
        let base = Url::parse("https://example.com/path/page").unwrap();

        assert_eq!(
            resolve_url("/favicon.ico", &base),
            Some("https://example.com/favicon.ico".to_string())
        );
        assert_eq!(
            resolve_url("icon.png", &base),
            Some("https://example.com/path/icon.png".to_string())
        );
        assert_eq!(
            resolve_url("//cdn.example.com/icon.png", &base),
            Some("https://cdn.example.com/icon.png".to_string())
        );
        assert_eq!(
            resolve_url("https://other.com/icon.png", &base),
            Some("https://other.com/icon.png".to_string())
        );
    }

    #[test]
    fn test_extract_favicon_from_html() {
        let base = Url::parse("https://example.com").unwrap();

        let html = r#"
            <html>
            <head>
                <link rel="icon" href="/static/favicon.ico">
            </head>
            </html>
        "#;
        assert_eq!(
            extract_favicon_from_html(html, &base),
            Some("https://example.com/static/favicon.ico".to_string())
        );

        let html2 = r#"<link rel="shortcut icon" href="https://cdn.example.com/icon.png">"#;
        assert_eq!(
            extract_favicon_from_html(html2, &base),
            Some("https://cdn.example.com/icon.png".to_string())
        );
    }
}
