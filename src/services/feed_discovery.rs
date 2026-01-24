use scraper::{Html, Selector};
use url::Url;

use crate::error::{AppError, AppResult};

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

    // Fetch the URL
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(user_agent)
        .build()
        .map_err(|e| AppError::FetchError(e.to_string()))?;

    let response = client
        .get(url)
        .send()
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
    let feed_response = client
        .get(&feed_url)
        .send()
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
