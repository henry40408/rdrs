use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;
use std::io::Cursor;

use crate::error::{AppError, AppResult};
use crate::models::{category::Category, feed::Feed};

/// Decode HTML entities in a string (e.g., &amp; -> &)
fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

#[derive(Debug, Clone)]
pub struct OpmlFeed {
    pub title: Option<String>,
    pub xml_url: String,
    pub html_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OpmlOutline {
    pub category_name: String,
    pub feeds: Vec<OpmlFeed>,
}

pub fn export_opml(categories: &[Category], feeds: &[Feed]) -> String {
    let mut writer = Writer::new(Cursor::new(Vec::new()));

    // XML declaration
    writer
        .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
        .unwrap();
    writer
        .write_event(Event::Text(BytesText::new("\n")))
        .unwrap();

    // OPML root element
    let mut opml = BytesStart::new("opml");
    opml.push_attribute(("version", "2.0"));
    writer.write_event(Event::Start(opml)).unwrap();
    writer
        .write_event(Event::Text(BytesText::new("\n")))
        .unwrap();

    // Head section
    writer
        .write_event(Event::Start(BytesStart::new("head")))
        .unwrap();
    writer
        .write_event(Event::Text(BytesText::new("\n")))
        .unwrap();
    writer
        .write_event(Event::Start(BytesStart::new("title")))
        .unwrap();
    writer
        .write_event(Event::Text(BytesText::new("RDRS Subscriptions")))
        .unwrap();
    writer
        .write_event(Event::End(BytesEnd::new("title")))
        .unwrap();
    writer
        .write_event(Event::Text(BytesText::new("\n")))
        .unwrap();
    writer
        .write_event(Event::End(BytesEnd::new("head")))
        .unwrap();
    writer
        .write_event(Event::Text(BytesText::new("\n")))
        .unwrap();

    // Body section
    writer
        .write_event(Event::Start(BytesStart::new("body")))
        .unwrap();
    writer
        .write_event(Event::Text(BytesText::new("\n")))
        .unwrap();

    // Group feeds by category
    for cat in categories {
        let cat_feeds: Vec<&Feed> = feeds.iter().filter(|f| f.category_id == cat.id).collect();

        // Skip empty categories
        if cat_feeds.is_empty() {
            continue;
        }

        // Category outline
        let mut cat_outline = BytesStart::new("outline");
        let decoded_cat_name = decode_html_entities(&cat.name);
        cat_outline.push_attribute(("text", decoded_cat_name.as_str()));
        cat_outline.push_attribute(("title", decoded_cat_name.as_str()));
        writer.write_event(Event::Start(cat_outline)).unwrap();
        writer
            .write_event(Event::Text(BytesText::new("\n")))
            .unwrap();

        // Feed outlines
        for feed in cat_feeds {
            let mut feed_outline = BytesStart::new("outline");
            feed_outline.push_attribute(("type", "rss"));

            let title = feed.title.as_deref().unwrap_or(&feed.url);
            let decoded_title = decode_html_entities(title);
            let decoded_url = decode_html_entities(&feed.url);

            feed_outline.push_attribute(("text", decoded_title.as_str()));
            feed_outline.push_attribute(("title", decoded_title.as_str()));
            feed_outline.push_attribute(("xmlUrl", decoded_url.as_str()));

            if let Some(site_url) = &feed.site_url {
                let decoded_site_url = decode_html_entities(site_url);
                feed_outline.push_attribute(("htmlUrl", decoded_site_url.as_str()));
            }

            writer.write_event(Event::Empty(feed_outline)).unwrap();
            writer
                .write_event(Event::Text(BytesText::new("\n")))
                .unwrap();
        }

        writer
            .write_event(Event::End(BytesEnd::new("outline")))
            .unwrap();
        writer
            .write_event(Event::Text(BytesText::new("\n")))
            .unwrap();
    }

    writer
        .write_event(Event::End(BytesEnd::new("body")))
        .unwrap();
    writer
        .write_event(Event::Text(BytesText::new("\n")))
        .unwrap();
    writer
        .write_event(Event::End(BytesEnd::new("opml")))
        .unwrap();

    let result = writer.into_inner().into_inner();
    String::from_utf8(result).unwrap_or_default()
}

pub fn parse_opml(content: &str) -> AppResult<Vec<OpmlOutline>> {
    let mut reader = Reader::from_str(content);
    reader.config_mut().trim_text(true);

    let mut outlines: Vec<OpmlOutline> = Vec::new();
    let mut current_category: Option<String> = None;
    let mut current_feeds: Vec<OpmlFeed> = Vec::new();
    let mut in_body = false;
    let mut depth = 0;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag_name = e.name();

                if tag_name.as_ref() == b"body" {
                    in_body = true;
                    continue;
                }

                if !in_body || tag_name.as_ref() != b"outline" {
                    continue;
                }

                // Parse attributes
                let mut text: Option<String> = None;
                let mut title: Option<String> = None;
                let mut xml_url: Option<String> = None;
                let mut html_url: Option<String> = None;

                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref()).to_lowercase();
                    let value = attr.decode_and_unescape_value(reader.decoder())
                        .map(|v| v.to_string())
                        .unwrap_or_else(|_| String::from_utf8_lossy(&attr.value).to_string());

                    match key.as_str() {
                        "text" => text = Some(value),
                        "title" => title = Some(value),
                        "xmlurl" => xml_url = Some(value),
                        "htmlurl" => html_url = Some(value),
                        _ => {}
                    }
                }

                // Determine if this is a feed or category
                if let Some(url) = xml_url {
                    // This is a feed (Start element with xmlUrl - unusual but handle it)
                    let feed = OpmlFeed {
                        title: title.or(text),
                        xml_url: url,
                        html_url,
                    };

                    if current_category.is_some() {
                        current_feeds.push(feed);
                    } else {
                        outlines.push(OpmlOutline {
                            category_name: "Uncategorized".to_string(),
                            feeds: vec![feed],
                        });
                    }
                } else {
                    // This is a category (Start outline without xmlUrl)
                    // Save previous category if exists
                    if let Some(cat_name) = current_category.take() {
                        if !current_feeds.is_empty() {
                            outlines.push(OpmlOutline {
                                category_name: cat_name,
                                feeds: std::mem::take(&mut current_feeds),
                            });
                        }
                    }

                    current_category = text.or(title);
                    depth += 1;
                }
            }
            Ok(Event::Empty(e)) => {
                let tag_name = e.name();

                if !in_body || tag_name.as_ref() != b"outline" {
                    continue;
                }

                // Parse attributes
                let mut text: Option<String> = None;
                let mut title: Option<String> = None;
                let mut xml_url: Option<String> = None;
                let mut html_url: Option<String> = None;

                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref()).to_lowercase();
                    let value = attr.decode_and_unescape_value(reader.decoder())
                        .map(|v| v.to_string())
                        .unwrap_or_else(|_| String::from_utf8_lossy(&attr.value).to_string());

                    match key.as_str() {
                        "text" => text = Some(value),
                        "title" => title = Some(value),
                        "xmlurl" => xml_url = Some(value),
                        "htmlurl" => html_url = Some(value),
                        _ => {}
                    }
                }

                // Empty outline - must be a feed (self-closing tag)
                if let Some(url) = xml_url {
                    let feed = OpmlFeed {
                        title: title.or(text),
                        xml_url: url,
                        html_url,
                    };

                    if current_category.is_some() {
                        current_feeds.push(feed);
                    } else {
                        outlines.push(OpmlOutline {
                            category_name: "Uncategorized".to_string(),
                            feeds: vec![feed],
                        });
                    }
                }
                // Empty outline without xmlUrl is ignored (empty category)
            }
            Ok(Event::End(e)) => {
                if e.name().as_ref() == b"body" {
                    in_body = false;
                } else if e.name().as_ref() == b"outline" && depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        // End of category
                        if let Some(cat_name) = current_category.take() {
                            if !current_feeds.is_empty() {
                                outlines.push(OpmlOutline {
                                    category_name: cat_name,
                                    feeds: std::mem::take(&mut current_feeds),
                                });
                            }
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(AppError::OpmlParseError(format!(
                    "Error at position {}: {:?}",
                    reader.error_position(),
                    e
                )));
            }
            _ => {}
        }
    }

    // Handle any remaining category
    if let Some(cat_name) = current_category {
        if !current_feeds.is_empty() {
            outlines.push(OpmlOutline {
                category_name: cat_name,
                feeds: current_feeds,
            });
        }
    }

    if outlines.is_empty() {
        return Err(AppError::OpmlParseError(
            "No feeds found in OPML".to_string(),
        ));
    }

    Ok(outlines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_export_opml() {
        let categories = vec![Category {
            id: 1,
            user_id: 1,
            name: "Tech".to_string(),
            created_at: Utc::now(),
        }];

        let feeds = vec![Feed {
            id: 1,
            category_id: 1,
            url: "https://blog.rust-lang.org/feed.xml".to_string(),
            title: Some("Rust Blog".to_string()),
            description: None,
            site_url: Some("https://blog.rust-lang.org".to_string()),
            feed_updated_at: None,
            fetched_at: None,
            fetch_error: None,
            etag: None,
            last_modified: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }];

        let opml = export_opml(&categories, &feeds);

        assert!(opml.contains("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(opml.contains("<opml version=\"2.0\">"));
        assert!(opml.contains("RDRS Subscriptions"));
        assert!(opml.contains("text=\"Tech\""));
        assert!(opml.contains("xmlUrl=\"https://blog.rust-lang.org/feed.xml\""));
        assert!(opml.contains("htmlUrl=\"https://blog.rust-lang.org\""));
    }

    #[test]
    fn test_parse_opml_basic() {
        let opml = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>Test</title></head>
  <body>
    <outline text="Tech" title="Tech">
      <outline type="rss" text="Rust Blog" title="Rust Blog"
               xmlUrl="https://blog.rust-lang.org/feed.xml"
               htmlUrl="https://blog.rust-lang.org"/>
    </outline>
  </body>
</opml>"#;

        let result = parse_opml(opml).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].category_name, "Tech");
        assert_eq!(result[0].feeds.len(), 1);
        assert_eq!(
            result[0].feeds[0].xml_url,
            "https://blog.rust-lang.org/feed.xml"
        );
        assert_eq!(result[0].feeds[0].title, Some("Rust Blog".to_string()));
    }

    #[test]
    fn test_parse_opml_multiple_categories() {
        let opml = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>Test</title></head>
  <body>
    <outline text="Tech">
      <outline type="rss" text="Feed 1" xmlUrl="https://example.com/1"/>
    </outline>
    <outline text="News">
      <outline type="rss" text="Feed 2" xmlUrl="https://example.com/2"/>
      <outline type="rss" text="Feed 3" xmlUrl="https://example.com/3"/>
    </outline>
  </body>
</opml>"#;

        let result = parse_opml(opml).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].category_name, "Tech");
        assert_eq!(result[0].feeds.len(), 1);
        assert_eq!(result[1].category_name, "News");
        assert_eq!(result[1].feeds.len(), 2);
    }

    #[test]
    fn test_parse_opml_invalid() {
        let opml = r#"<not-opml></not-opml>"#;
        let result = parse_opml(opml);
        assert!(result.is_err());
    }
}
