//! Golden-vector generator for the Go port. Ignored by default; run with
//! `cargo nextest run --run-ignored only go_vectors` to rewrite
//! `testdata/vectors/*.json` from the Rust implementation.

use std::fs;
use std::path::Path;

use chrono::{TimeZone, Utc};
use serde_json::{Value, json};

use rdrs::models::feed::url_to_bucket;
use rdrs::secret;
use rdrs::services::image_proxy;
use rdrs::services::{decode_html_entities, parse_opml, strip_tracking_params};
use rdrs::utils::{datetime, han, text};

const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

fn write(name: &str, value: &Value) {
    let dir = Path::new("testdata/vectors");
    fs::create_dir_all(dir).unwrap();
    let body = serde_json::to_string_pretty(value).unwrap() + "\n";
    fs::write(dir.join(format!("{name}.json")), body).unwrap();
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_secret() {
    let tokens = ["", "abc", "token-with.dot", "日本語"];
    let tags: Vec<Value> = tokens
        .iter()
        .map(|t| {
            json!({
                "token": t,
                "tag_image": hex::encode(secret::tag(SECRET, secret::DOMAIN_IMAGE, &[t.as_bytes()])),
                "session_cookie": secret::sign_session(SECRET, t),
                "csrf": secret::derive_csrf(SECRET, t),
                "audit_id": secret::audit_id(SECRET, t),
            })
        })
        .collect();
    let ids: Vec<Value> = [0i64, 1, 7, -1, i64::MAX]
        .iter()
        .map(|id| {
            json!({
                "id": id,
                "offline_id": secret::offline_id(SECRET, *id),
                "pixel_sig_entry_42": secret::pixel_sig(SECRET, *id, 42),
            })
        })
        .collect();
    let sealed = secret::seal(SECRET, "hunter2-token");
    write(
        "secret",
        &json!({
            "secret": String::from_utf8_lossy(SECRET),
            "tags": tags,
            "ids": ids,
            "sealed": { "plaintext": "hunter2-token", "stored": sealed },
        }),
    );
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_feed_bucket() {
    let urls = [
        "",
        "https://example.com/feed.xml",
        "https://example.org/rss",
        "http://blog.example.net/atom?x=1",
        "https://日本語.example/フィード",
    ];
    let rows: Vec<Value> = urls
        .iter()
        .map(|u| json!({ "url": u, "bucket": url_to_bucket(u) }))
        .collect();
    write("feed_bucket", &Value::Array(rows));
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_image_proxy() {
    let urls = [
        "https://example.com/a.png",
        "https://example.com/a.png?x=1&y=2",
    ];
    let rows: Vec<Value> = urls
        .iter()
        .map(|u| {
            json!({
                "url": u,
                "sign": image_proxy::sign_url(u, SECRET),
                "proxy_relative": image_proxy::create_proxy_url(u, SECRET, None),
                "proxy_absolute": image_proxy::create_proxy_url(u, SECRET, Some("https://rdrs.example/")),
                "sign_referrer": image_proxy::sign_url_with_referrer(u, "https://ref.example/", SECRET),
                "proxy_referrer": image_proxy::create_proxy_url_with_referrer(
                    u, "https://ref.example/", SECRET, None),
            })
        })
        .collect();
    write("image_proxy", &Value::Array(rows));
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_text() {
    let inputs = [
        "plain",
        "<p>Hello <b>world</b></p>",
        "a &amp; b &lt;c&gt; &#39;q&#39; &#x27;h&#x27; &nbsp; &bogus; &amp;#x27;",
        "<script>x()</script><style>p{}</style>text",
        "line1<br>line2<p>para</p>",
        "  spaced \n\n out\t text  ",
        "日本語<em>テスト</em>",
    ];
    let rows: Vec<Value> = inputs
        .iter()
        .map(|i| {
            json!({
                "input": i,
                "plain": text::strip_to_plain_text(i),
                "search": text::strip_to_search_text(i),
                "decoded": decode_html_entities(i),
                "han_lang": han::lang_attr(i),
            })
        })
        .collect();
    let han: Vec<Value> = ["这是简体中文测试", "這是繁體中文測試", "hello", "国国国国国国国国国国国国"]
        .iter()
        .map(|i| json!({ "input": i, "lang": han::lang_attr(i) }))
        .collect();
    write("text", &json!({ "rows": rows, "han": han }));
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_datetime() {
    let inputs = [
        "2024-03-05T10:20:30Z",
        "2024-03-05T10:20:30+08:00",
        "2024-03-05 10:20:30",
        "Tue, 05 Mar 2024 10:20:30 GMT",
        "Tue, 05 Mar 2024 10:20:30 +0800",
        "5 Mar 2024",
        "2024-03-05",
        "週二, 05 三月 2024 10:20:30 +0800",
        "05 三月 2024 10:20:30",
        "not a date",
        "",
    ];
    let rows: Vec<Value> = inputs
        .iter()
        .map(|i| {
            json!({
                "input": i,
                "normalized": datetime::normalize_timezone_format(i),
                "parsed": datetime::try_parse_datetime(i).map(datetime::format_display),
            })
        })
        .collect();
    let at = Utc.with_ymd_and_hms(2024, 3, 5, 10, 20, 30).unwrap();
    write(
        "datetime",
        &json!({ "rows": rows, "display": datetime::format_display(at) }),
    );
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_tracking_params() {
    let urls = [
        "https://example.com/a?utm_source=x&id=1",
        "https://example.com/a?fbclid=abc",
        "https://example.com/a#frag",
        "https://example.com/a?b=2&a=1&utm_medium=m",
        "not a url",
    ];
    let rows: Vec<Value> = urls
        .iter()
        .map(|u| json!({ "url": u, "stripped": strip_tracking_params(u) }))
        .collect();
    write("tracking_params", &Value::Array(rows));
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_opml() {
    let content = fs::read_to_string("e2e/fixtures/sample.opml").unwrap();
    let outlines = parse_opml(&content).unwrap();
    let rows: Vec<Value> = outlines
        .iter()
        .map(|o| {
            json!({
                "category": o.category_name,
                "feeds": o.feeds.iter().map(|f| json!({
                    "title": f.title, "xml_url": f.xml_url, "html_url": f.html_url,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    write("opml_parse", &Value::Array(rows));
}
