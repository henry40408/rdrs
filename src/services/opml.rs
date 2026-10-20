use std::fmt::Write as _;

use quick_xml::XmlVersion;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;
use std::io::Cursor;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::models::{category, category::Category, feed, feed::Feed};
use crate::services::fetch::Fetcher;
use crate::services::html_entities::decode_html_entities;
use url::Url;

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

    writer
        .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
        .unwrap();
    writer
        .write_event(Event::Text(BytesText::new("\n")))
        .unwrap();

    let mut opml = BytesStart::new("opml");
    opml.push_attribute(("version", "2.0"));
    writer.write_event(Event::Start(opml)).unwrap();
    writer
        .write_event(Event::Text(BytesText::new("\n")))
        .unwrap();

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

    writer
        .write_event(Event::Start(BytesStart::new("body")))
        .unwrap();
    writer
        .write_event(Event::Text(BytesText::new("\n")))
        .unwrap();

    // Group feeds by category
    for cat in categories {
        let cat_feeds: Vec<&Feed> = feeds.iter().filter(|f| f.category_id == cat.id).collect();

        if cat_feeds.is_empty() {
            continue;
        }

        let mut cat_outline = BytesStart::new("outline");
        let decoded_cat_name = decode_html_entities(&cat.name);
        cat_outline.push_attribute(("text", decoded_cat_name.as_str()));
        cat_outline.push_attribute(("title", decoded_cat_name.as_str()));
        writer.write_event(Event::Start(cat_outline)).unwrap();
        writer
            .write_event(Event::Text(BytesText::new("\n")))
            .unwrap();

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

#[derive(Default)]
struct OutlineAttributes {
    text: Option<String>,
    title: Option<String>,
    xml_url: Option<String>,
    html_url: Option<String>,
}

fn outline_attributes(element: &BytesStart) -> OutlineAttributes {
    let mut attributes = OutlineAttributes::default();

    for attr in element.attributes().flatten() {
        let key = attr.key.as_ref().to_lowercase();
        let value = attr
            .normalized_value(XmlVersion::Implicit1_0)
            .map_or_else(|_| attr.value.to_string(), |v| v.to_string());

        match key.as_str() {
            "text" => attributes.text = Some(value),
            "title" => attributes.title = Some(value),
            "xmlurl" => attributes.xml_url = Some(value),
            "htmlurl" => attributes.html_url = Some(value),
            _ => {}
        }
    }

    attributes
}

/// Files the feed under the open category, or on its own under "Uncategorized".
fn push_feed(
    outlines: &mut Vec<OpmlOutline>,
    category_is_open: bool,
    current_feeds: &mut Vec<OpmlFeed>,
    feed: OpmlFeed,
) {
    if category_is_open {
        current_feeds.push(feed);
    } else {
        outlines.push(OpmlOutline {
            category_name: "Uncategorized".to_string(),
            feeds: vec![feed],
        });
    }
}

/// Closes the open category; one without feeds is dropped.
fn flush_category(
    outlines: &mut Vec<OpmlOutline>,
    category: Option<String>,
    current_feeds: &mut Vec<OpmlFeed>,
) {
    if let Some(category_name) = category
        && !current_feeds.is_empty()
    {
        outlines.push(OpmlOutline {
            category_name,
            feeds: std::mem::take(current_feeds),
        });
    }
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

                if tag_name.as_ref() == "body" {
                    in_body = true;
                    continue;
                }

                if !in_body || tag_name.as_ref() != "outline" {
                    continue;
                }

                let attributes = outline_attributes(&e);

                if let Some(url) = attributes.xml_url {
                    // Start element with xmlUrl: unusual, but a feed.
                    let feed = OpmlFeed {
                        title: attributes.title.or(attributes.text),
                        xml_url: url,
                        html_url: attributes.html_url,
                    };
                    push_feed(
                        &mut outlines,
                        current_category.is_some(),
                        &mut current_feeds,
                        feed,
                    );
                } else {
                    flush_category(&mut outlines, current_category.take(), &mut current_feeds);
                    current_category = attributes.text.or(attributes.title);
                    depth += 1;
                }
            }
            Ok(Event::Empty(e)) => {
                if !in_body || e.name().as_ref() != "outline" {
                    continue;
                }

                let attributes = outline_attributes(&e);

                // Without xmlUrl a self-closing outline is an empty category, so it is ignored.
                if let Some(url) = attributes.xml_url {
                    let feed = OpmlFeed {
                        title: attributes.title.or(attributes.text),
                        xml_url: url,
                        html_url: attributes.html_url,
                    };
                    push_feed(
                        &mut outlines,
                        current_category.is_some(),
                        &mut current_feeds,
                        feed,
                    );
                }
            }
            Ok(Event::End(e)) => {
                if e.name().as_ref() == "body" {
                    in_body = false;
                } else if e.name().as_ref() == "outline" && depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        flush_category(&mut outlines, current_category.take(), &mut current_feeds);
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

    flush_category(&mut outlines, current_category, &mut current_feeds);

    if outlines.is_empty() {
        return Err(AppError::OpmlParseError(
            "No feeds found in OPML".to_string(),
        ));
    }

    Ok(outlines)
}

/// What an OPML import did, reported back so "imported" distinguishes 300 new
/// feeds from all-duplicates.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ImportSummary {
    pub categories_created: usize,
    pub feeds_added: usize,
    /// Already subscribed in the same category, so left alone.
    pub feeds_skipped: usize,
    /// Rejected by the database (duplicate URL race, oversized field, …).
    pub feeds_failed: usize,
}

impl ImportSummary {
    /// One-line summary for a flash message; zero-count clauses are omitted.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut out = format!(
            "OPML imported: {} feed{} added",
            self.feeds_added,
            if self.feeds_added == 1 { "" } else { "s" }
        );
        if self.feeds_skipped > 0 {
            let _ = write!(out, ", {} already subscribed", self.feeds_skipped);
        }
        if self.feeds_failed > 0 {
            let _ = write!(out, ", {} failed", self.feeds_failed);
        }
        out.push('.');
        out
    }
}

/// Subscribe `user_id` to every feed in `outlines`, creating missing
/// categories. Duplicates are skipped and DB rejections counted, so one bad
/// entry cannot sink the run; only a category failure is fatal.
pub async fn import_outlines(
    db: &Db,
    user_id: i64,
    outlines: Vec<OpmlOutline>,
    fetcher: &Fetcher,
) -> AppResult<ImportSummary> {
    let mut summary = ImportSummary::default();

    for outline in outlines {
        let existing = category::find_by_name_and_user(db, &outline.category_name, user_id).await?;
        let cat = if let Some(cat) = existing {
            cat
        } else {
            summary.categories_created += 1;
            category::create_category(db, user_id, &outline.category_name).await?
        };

        for opml_feed in outline.feeds {
            // OPML is attacker-influenced like a typed URL; validate before storing.
            let allowed =
                Url::parse(&opml_feed.xml_url).is_ok_and(|url| fetcher.validate(&url).is_ok());
            if !allowed {
                summary.feeds_failed += 1;
                tracing::warn!(
                    event = "opml.feed_import_rejected",
                    url = %opml_feed.xml_url,
                    "skipping OPML feed whose URL is not fetchable"
                );
                continue;
            }

            if feed::find_by_url_and_category(db, &opml_feed.xml_url, cat.id)
                .await?
                .is_some()
            {
                summary.feeds_skipped += 1;
                continue;
            }

            let created = feed::create_feed(
                db,
                &feed::CreateFeedParams {
                    category_id: cat.id,
                    url: &opml_feed.xml_url,
                    title: opml_feed.title.as_deref(),
                    description: None,
                    site_url: opml_feed.html_url.as_deref(),
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .await;

            match created {
                Ok(_) => summary.feeds_added += 1,
                Err(e) => {
                    summary.feeds_failed += 1;
                    tracing::warn!(
                        event = "opml.feed_import_failed",
                        url = %opml_feed.xml_url,
                        error = %e,
                        "skipping OPML feed the database rejected"
                    );
                }
            }
        }
    }

    Ok(summary)
}

#[cfg(test)]
mod import_tests {
    use super::*;
    use crate::models::user::{self, Role};
    use crate::services::fetch::Fetcher;

    /// OPML `xmlUrl`s are no more trusted than a typed URL.
    #[tokio::test]
    async fn import_rejects_feeds_pointing_at_private_addresses() {
        let db = Db::connect_in_memory().await.unwrap();
        let u = user::create_user(&db, "opmluser", "hash", Role::User)
            .await
            .unwrap();

        let outlines = vec![OpmlOutline {
            category_name: "Imported".to_string(),
            feeds: vec![
                OpmlFeed {
                    title: Some("Public".to_string()),
                    xml_url: "https://example.com/feed.xml".to_string(),
                    html_url: None,
                },
                OpmlFeed {
                    title: Some("Metadata service".to_string()),
                    xml_url: "http://169.254.169.254/latest/meta-data/".to_string(),
                    html_url: None,
                },
                OpmlFeed {
                    title: Some("Not a URL".to_string()),
                    xml_url: "file:///etc/passwd".to_string(),
                    html_url: None,
                },
            ],
        }];

        let summary = import_outlines(&db, u.id, outlines, &Fetcher::default())
            .await
            .unwrap();

        assert_eq!(summary.feeds_added, 1);
        assert_eq!(summary.feeds_failed, 2);

        let stored = feed::list_by_user(&db, u.id).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].url, "https://example.com/feed.xml");
    }
}

#[cfg(test)]
mod summary_tests {
    use super::ImportSummary;

    #[test]
    fn describe_omits_zero_clauses() {
        let s = ImportSummary {
            feeds_added: 12,
            ..ImportSummary::default()
        };
        assert_eq!(s.describe(), "OPML imported: 12 feeds added.");
    }

    #[test]
    fn describe_uses_singular_for_one_feed() {
        let s = ImportSummary {
            feeds_added: 1,
            ..ImportSummary::default()
        };
        assert_eq!(s.describe(), "OPML imported: 1 feed added.");
    }

    #[test]
    fn describe_reports_a_no_op_import_honestly() {
        let s = ImportSummary {
            feeds_skipped: 40,
            ..ImportSummary::default()
        };
        assert_eq!(
            s.describe(),
            "OPML imported: 0 feeds added, 40 already subscribed."
        );
    }

    #[test]
    fn describe_surfaces_failures() {
        let s = ImportSummary {
            categories_created: 2,
            feeds_added: 5,
            feeds_skipped: 1,
            feeds_failed: 3,
        };
        assert_eq!(
            s.describe(),
            "OPML imported: 5 feeds added, 1 already subscribed, 3 failed."
        );
    }
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
            custom_user_agent: None,
            http2_disabled: false,
            custom_referrer: None,
            bucket: None,
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

    /// `&amp;` in an attribute must be unescaped; pins the result across
    /// quick-xml API changes.
    #[test]
    fn test_parse_opml_unescapes_attribute_entities() {
        let opml = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>Test</title></head>
  <body>
    <outline text="News &amp; Views">
      <outline type="rss" text="Tom &amp; Jerry"
               xmlUrl="https://example.com/feed?a=1&amp;b=2"
               htmlUrl="https://example.com/?x=1&amp;y=2"/>
    </outline>
  </body>
</opml>"#;

        let result = parse_opml(opml).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].category_name, "News & Views");
        assert_eq!(result[0].feeds.len(), 1);
        assert_eq!(
            result[0].feeds[0].xml_url,
            "https://example.com/feed?a=1&b=2"
        );
        assert_eq!(
            result[0].feeds[0].html_url,
            Some("https://example.com/?x=1&y=2".to_string())
        );
        assert_eq!(result[0].feeds[0].title, Some("Tom & Jerry".to_string()));
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
    fn test_parse_opml_uncategorized_nested_and_start_feeds() {
        let opml = r#"<opml version="2.0">
  <body>
    <outline type="rss" text="Loose" xmlUrl="https://example.com/loose"/>
    <outline type="rss" text="Open" xmlUrl="https://example.com/open"></outline>
    <outline text="Empty category"/>
    <outline text="Outer">
      <outline type="rss" title="Before" xmlUrl="https://example.com/before"/>
      <outline text="Inner">
        <outline type="rss" text="Inside" xmlUrl="https://example.com/inside"/>
      </outline>
    </outline>
    <outline text="No feeds"></outline>
  </body>
</opml>"#;

        let result = parse_opml(opml).unwrap();
        let shape: Vec<(&str, Vec<&str>)> = result
            .iter()
            .map(|o| {
                (
                    o.category_name.as_str(),
                    o.feeds.iter().map(|f| f.xml_url.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                ("Uncategorized", vec!["https://example.com/loose"]),
                ("Uncategorized", vec!["https://example.com/open"]),
                ("Outer", vec!["https://example.com/before"]),
                ("Inner", vec!["https://example.com/inside"]),
            ]
        );
    }

    #[test]
    fn test_parse_opml_prefers_text_for_categories_and_title_for_feeds() {
        let opml = r#"<opml version="2.0">
  <body>
    <outline text="Category text" title="Category title">
      <outline type="rss" text="Feed text" title="Feed title" xmlUrl="https://example.com/a"/>
      <outline type="rss" text="Open text" title="Open title" xmlUrl="https://example.com/b"></outline>
    </outline>
  </body>
</opml>"#;

        let result = parse_opml(opml).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].category_name, "Category text");
        let titles: Vec<_> = result[0].feeds.iter().map(|f| f.title.as_deref()).collect();
        assert_eq!(titles, vec![Some("Feed title"), Some("Open title")]);
    }

    #[test]
    fn test_parse_opml_invalid() {
        let opml = r"<not-opml></not-opml>";
        let result = parse_opml(opml);
        assert!(result.is_err());
    }
}
