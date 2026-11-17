//! Golden-vector generator for the Go port. Ignored by default; run with
//! `cargo nextest run --run-ignored only go_vectors` to rewrite
//! `testdata/vectors/*.json` from the Rust implementation.

use std::fs;
use std::path::Path;

use chrono::{TimeZone, Utc};
use serde_json::{Value, json};

use rdrs::auth::password;
use rdrs::models::category::Category;
use rdrs::models::feed::{Feed, url_to_bucket};
use rdrs::secret;
use rdrs::services::image_proxy;
use rdrs::services::pixel::PixelContext;
use rdrs::services::{
    SanitizeContext, decode_html_entities, export_opml, parse_opml, sanitize_html,
    sanitize_summary, strip_tracking_params,
};
use rdrs::utils::{datetime, han, text, timezone};

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
    let han: Vec<Value> = [
        "这是简体中文测试",
        "這是繁體中文測試",
        "hello",
        "国国国国国国国国国国国国",
    ]
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

fn read_json(path: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_sanitize() {
    const SANITIZE_SECRET: &[u8] = b"test_secret_key_32_bytes_long!!!";
    let mut inputs: Vec<String> = read_json("testdata/vectors/sanitize_inputs.json")
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    inputs.extend(
        [
            "",
            "plain text & <angle> \"quotes\" 'apos'",
            "<p>nbsp\u{a0}here &nbsp; and &copy; &#169; &#xA9;</p>",
            "<p>unclosed <b>bold <i>italic",
            "<table><tr><td>cell</td></tr></table>",
            "<div><span lang=\"fr\" title=\"t\" class=\"c\" id=\"i\" style=\"color:red\">x</span></div>",
            "<a href=\"HTTPS://Example.com/Path?utm_source=a&amp;id=2#f\">link</a>",
            "<a href=\"mailto:a@b.c\">mail</a><a href=\"ftp://x/y\">ftp</a><a href=\"//cdn.example/x\">proto</a>",
            "<img src=\"/rel.png\" alt=\"a\" title=\"t\" width=\"100\" height=\"50\">",
            "<img src=\"https://example.com/a.png\" srcset=\"a.png 1x\" sizes=\"10px\" loading=\"eager\">",
            "<p aria-hidden=\"true\">hidden</p><p>shown</p>",
            "<div aria-hidden=\"true\"><p>all hidden</p></div>",
            "<img data-src=\"https://example.com/l.png\" width=\"1\" height=\"1\">",
            "<img style=\"width:1px;height:1px\" src=\"https://example.com/x.png\">",
            "<img src=\"https://pixel.example.com/x.gif\"><img src=\"https://example.com/track/x.gif\">",
            "<blockquote cite=\"https://example.com/q\">q</blockquote><blockquote cite=\"javascript:x\">q</blockquote>",
            "<pre><code>a &lt; b\n  indented</code></pre>",
            "<ul><li>a<li>b</ul><ol start=\"3\" type=\"a\"><li>c</ol>",
            "<h1>h</h1><h2>h</h2><h6>h</h6><hr><center>c</center><font color=\"red\">f</font>",
            "<figure><img src=\"https://example.com/f.png\"><figcaption>cap</figcaption></figure>",
            "<br/><br><p/>",
            "<!-- comment --><p>after</p><![CDATA[cdata]]>",
            "<svg><circle/></svg><math><mi>x</mi></math><iframe src=\"https://e/\"></iframe><object data=\"x\"></object>",
            "<form action=\"/x\"><input name=\"a\"><button>b</button></form>",
            "<p onclick=\"x()\" onmouseover=\"y()\">events</p>",
            "<a href=\"https://example.com\" target=\"_self\" rel=\"nofollow\" onclick=\"x\">a</a>",
            "<td>orphan cell</td><tr><th>orphan row</th></tr>",
            "<video src=\"v.mp4\"><source src=\"s.mp4\"></video><audio src=\"a.mp3\"></audio>",
            "<p>日本語と<b>中文</b></p>",
            "<a href=\"https://example.com/?ref=x&amp;fbclid=y&amp;gclid=z&amp;keep=1\">c</a>",
            "<img src=\"https://example.com/a.png?x=1&amp;y=2\">",
            "<img src=\"data:image/png;base64,AAAA\">",
        ]
        .map(String::from),
    );

    let contexts: [(&str, Option<&str>, Option<&str>, Option<&str>); 3] = [
        ("plain", None, None, None),
        ("base", Some("https://example.com/blog/post"), None, None),
        (
            "full",
            Some("https://example.com/blog/post"),
            Some("https://ref.example/"),
            Some("https://rdrs.example"),
        ),
    ];
    let mut rows = Vec::new();
    for input in &inputs {
        let mut outputs = serde_json::Map::new();
        for (name, base_url, referrer, proxy_base_url) in contexts {
            let ctx = SanitizeContext {
                secret: SANITIZE_SECRET,
                base_url,
                referrer,
                proxy_base_url,
            };
            outputs.insert(name.to_string(), json!(sanitize_html(input, &ctx)));
        }
        rows.push(json!({
            "input": input,
            "outputs": outputs,
            "summary": sanitize_summary(input),
        }));
    }
    write(
        "sanitize",
        &json!({ "secret": String::from_utf8_lossy(SANITIZE_SECRET), "rows": rows }),
    );
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_timezone() {
    let names = [
        "UTC",
        "Asia/Taipei",
        "America/New_York",
        "Pacific/Apia",
        "Asia/Kolkata",
        "Australia/Lord_Howe",
        "Europe/Kyiv",
        "Europe/Kiev",
        "US/Pacific",
        "EST5EDT",
        "Nope/Zone",
        "",
        " UTC",
        "utc",
    ];
    let parse: Vec<Value> = names
        .iter()
        .map(|n| {
            json!({ "name": n, "parsed": timezone::parse(n).map(|t| t.to_string()),
                    "or_utc": timezone::parse_or_utc(n).to_string() })
        })
        .collect();
    let instants = [
        Utc.with_ymd_and_hms(2024, 3, 10, 6, 59, 59).unwrap(),
        Utc.with_ymd_and_hms(2024, 3, 10, 7, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2024, 11, 3, 5, 59, 59).unwrap(),
        Utc.with_ymd_and_hms(2024, 11, 3, 6, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2011, 12, 30, 9, 59, 59).unwrap(),
        Utc.with_ymd_and_hms(2011, 12, 30, 10, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2024, 7, 1, 23, 30, 0).unwrap(),
    ];
    let zones = [
        "UTC",
        "Asia/Taipei",
        "America/New_York",
        "Pacific/Apia",
        "Australia/Lord_Howe",
    ];
    let mut date_at = Vec::new();
    for z in zones {
        let tz = timezone::parse_or_utc(z);
        for at in instants {
            date_at.push(json!({ "tz": z, "at": at.to_rfc3339(),
                "date": timezone::date_at(at, tz).to_string() }));
        }
    }
    let dates = [
        (2024, 3, 10),
        (2024, 3, 11),
        (2024, 11, 3),
        (2024, 11, 4),
        (2011, 12, 29),
        (2011, 12, 30),
        (2011, 12, 31),
        (2024, 4, 7),
        (2024, 10, 6),
        (2024, 1, 1),
    ];
    let mut bounds = Vec::new();
    for z in zones {
        let tz = timezone::parse_or_utc(z);
        for (y, m, d) in dates {
            let date = chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap();
            let (s, e) = timezone::day_bounds_utc(date, tz);
            bounds.push(json!({
                "tz": z, "date": date.to_string(),
                "start_of_day": timezone::start_of_day_utc(date, tz).to_string(),
                "start": s.to_rfc3339(), "end": e.to_rfc3339(),
            }));
        }
    }
    let pairs = [
        ("UTC", "Europe/London"),
        ("Europe/London", "Europe/Dublin"),
        ("Asia/Taipei", "Asia/Shanghai"),
        ("America/New_York", "America/Toronto"),
        ("Europe/Kyiv", "Europe/Kiev"),
        ("UTC", "UTC"),
        ("Asia/Kolkata", "Asia/Calcutta"),
    ];
    let nows = [
        Utc.with_ymd_and_hms(2024, 1, 15, 12, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2024, 7, 15, 12, 0, 0).unwrap(),
    ];
    let mut same = Vec::new();
    for (a, b) in pairs {
        for now in nows {
            same.push(json!({ "a": a, "b": b, "now": now.to_rfc3339(),
                "same": timezone::same_rules(
                    timezone::parse_or_utc(a), timezone::parse_or_utc(b), now) }));
        }
    }
    write(
        "timezone",
        &json!({
            "default": timezone::DEFAULT_TIMEZONE,
            "suggestions": timezone::TIMEZONE_SUGGESTIONS,
            "parse": parse, "date_at": date_at, "day_bounds": bounds, "same_rules": same,
        }),
    );
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_pixel() {
    let opt_in = Utc.with_ymd_and_hms(2024, 3, 5, 0, 0, 0).unwrap();
    let before = Utc.with_ymd_and_hms(2024, 3, 4, 23, 59, 59).unwrap();
    let after = Utc.with_ymd_and_hms(2024, 3, 5, 0, 0, 1).unwrap();
    let mut urls = Vec::new();
    for (uid, eid) in [(7i64, 42i64), (0, 0), (1, i64::MAX), (-1, -2)] {
        for base in [
            None,
            Some("https://rdrs.example"),
            Some("https://rdrs.example/"),
        ] {
            let ctx = PixelContext {
                user_id: uid,
                enabled_at: Some(opt_in),
                secret: SECRET,
                base_url: base,
            };
            urls.push(json!({ "user_id": uid, "entry_id": eid, "base_url": base,
                "url": ctx.url(eid) }));
        }
    }
    let mut inject = Vec::new();
    for enabled in [None, Some(opt_in)] {
        for created in [before, opt_in, after] {
            for html in ["<p>Article</p>", "", "  \n ", "<p>x</p>"] {
                let ctx = PixelContext {
                    user_id: 7,
                    enabled_at: enabled,
                    secret: SECRET,
                    base_url: None,
                };
                inject.push(json!({
                    "enabled": enabled.is_some(), "created_at": created.to_rfc3339(),
                    "html": html,
                    "output": ctx.maybe_inject(html.to_string(), 42, created),
                }));
            }
        }
    }
    let gif: String = rdrs::services::pixel::TRANSPARENT_GIF
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    write(
        "pixel",
        &json!({
            "secret": String::from_utf8_lossy(SECRET),
            "prefix": rdrs::services::pixel::PIXEL_PATH_PREFIX,
            "opt_in": opt_in.to_rfc3339(), "urls": urls, "inject": inject, "gif_hex": gif,
        }),
    );
}

fn argon2_rows() -> Value {
    let passwords = [
        "correct horse battery staple",
        "日本語パスワード-15chars",
        "",
    ];
    let rows: Vec<Value> = passwords
        .iter()
        .map(|p| {
            let hash = password::hash_password(p).unwrap();
            json!({ "password": p, "hash": hash,
                "verifies": password::verify_password(p, &hash),
                "rejects_other": password::verify_password("wrong", &hash) })
        })
        .collect();
    json!(rows)
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_argon2_default() {
    let verify_cases: Vec<Value> = [
        "!",
        "",
        "not-a-hash",
        "$argon2id$v=19$m=8,t=1,p=1$c2FsdHNhbHQ$AAAA",
    ]
    .iter()
    .map(|h| json!({ "hash": h, "verifies": password::verify_password("x", h) }))
    .collect();
    write(
        "argon2_default",
        &json!({ "hashed": argon2_rows(), "verify_cases": verify_cases }),
    );
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_argon2_fast() {
    // The generator script sets RDRS_FAST_HASH=1 for every test but argon2_default.
    assert!(std::env::var_os("RDRS_FAST_HASH").is_some());
    write("argon2_fast", &json!({ "hashed": argon2_rows() }));
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_password_strength() {
    let cases: [(&str, &[&str]); 16] = [
        ("short", &[]),
        ("correct horse battery staple", &[]),
        ("passwordpassword", &[]),
        ("aaaaaaaaaaaaaaaa", &[]),
        ("123456789012345", &[]),
        ("qwertyuiopasdfgh", &[]),
        ("alice-the-admin-1", &["alice"]),
        ("alicealicealice", &["alice"]),
        ("Tr0ub4dor&3-and-more", &[]),
        ("日本語日本語日本語日本語日本語", &[]),
        ("日本語のパスワードです長い", &[]),
        ("abcdefghijklmno", &[]),
        ("xkcd-battery-horse-staple-9", &[]),
        ("monkey-monkey-monkey", &[]),
        ("letmeinletmeinletmein", &[]),
        ("Rr7!kQ2#vLp9@xZw4$", &[]),
    ];
    let long = "a".repeat(129);
    let mut rows: Vec<Value> = cases
        .iter()
        .map(|(p, u)| {
            json!({ "password": p, "user_inputs": u,
                "error": password::validate_password_strength(p, u).err().map(|e| e.to_string()) })
        })
        .collect();
    rows.push(json!({ "password": long, "user_inputs": [],
        "error": password::validate_password_strength(&long, &[]).err().map(|e| e.to_string()) }));
    write("zxcvbn", &json!(rows));
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_migrations() {
    use sha2::{Digest, Sha384};
    let mut out = serde_json::Map::new();
    for backend in ["sqlite", "postgres"] {
        let dir = format!("migrations/{backend}");
        let mut files: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        let rows: Vec<Value> = files
            .iter()
            .map(|p| {
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                let bytes = fs::read(p).unwrap();
                let (version, rest) = name.split_once('_').unwrap();
                json!({
                    "file": name,
                    "version": version.parse::<i64>().unwrap(),
                    "description": rest.trim_end_matches(".sql").replace('_', " "),
                    "sha384": hex::encode(Sha384::digest(&bytes)),
                })
            })
            .collect();
        out.insert(backend.to_string(), json!(rows));
    }
    write("migrations", &Value::Object(out));
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_opml_export() {
    let at = Utc.with_ymd_and_hms(2024, 3, 5, 10, 20, 30).unwrap();
    let cat = |id: i64, name: &str| Category {
        id,
        user_id: 1,
        name: name.into(),
        created_at: at,
    };
    let feed =
        |id: i64, category_id: i64, url: &str, title: Option<&str>, site: Option<&str>| Feed {
            id,
            category_id,
            url: url.into(),
            title: title.map(Into::into),
            description: None,
            site_url: site.map(Into::into),
            feed_updated_at: None,
            fetched_at: None,
            fetch_error: None,
            etag: None,
            last_modified: None,
            custom_user_agent: None,
            http2_disabled: false,
            custom_referrer: None,
            bucket: None,
            created_at: at,
            updated_at: at,
        };
    let categories = vec![
        cat(1, "Tech"),
        cat(2, "News &amp; Views"),
        cat(3, "Empty"),
        cat(4, "Q\"uote's <b>"),
    ];
    let feeds = vec![
        feed(
            1,
            1,
            "https://a.example/feed.xml",
            Some("A Feed"),
            Some("https://a.example/"),
        ),
        feed(2, 1, "https://b.example/rss?x=1&y=2", None, None),
        feed(
            3,
            2,
            "https://c.example/atom",
            Some("C &amp; D"),
            Some("https://c.example/?a=1&b=2"),
        ),
        feed(
            4,
            4,
            "https://d.example/\"q\"",
            Some("It's \"quoted\" <tag>"),
            None,
        ),
        feed(5, 2, "https://e.example/日本語", Some("日本語"), None),
    ];
    let cats_json: Vec<Value> = categories
        .iter()
        .map(|c| json!({ "id": c.id, "name": c.name }))
        .collect();
    let feeds_json: Vec<Value> = feeds
        .iter()
        .map(|f| {
            json!({ "id": f.id, "category_id": f.category_id, "url": f.url,
                "title": f.title, "site_url": f.site_url })
        })
        .collect();
    write(
        "opml_export",
        &json!({ "categories": cats_json, "feeds": feeds_json,
            "output": export_opml(&categories, &feeds),
            "empty_output": export_opml(&[], &[]) }),
    );

    let docs = [
        r#"<opml version="2.0"><body><outline text="Cat"><outline type="rss" text="F" xmlUrl="https://x/1"/></outline></body></opml>"#,
        r#"<opml><body><outline type="rss" text="Top" xmlUrl="https://x/top"/></body></opml>"#,
        r#"<opml><body><outline text="A"><outline text="B"><outline type="rss" text="F" xmlUrl="https://x/n" htmlUrl="https://x/"/></outline></outline></body></opml>"#,
        r#"<opml><body><outline title="OnlyTitle"><outline xmlUrl="https://x/notype"/></outline></body></opml>"#,
        r#"<opml><body><outline text="Dup"><outline xmlUrl="https://x/1"/><outline xmlUrl="https://x/1"/></outline><outline text="Dup"><outline xmlUrl="https://x/2"/></outline></body></opml>"#,
        r#"<opml><body><outline text="E &amp;amp; F"><outline text="T &amp;amp; U" xmlUrl="https://x/?a=1&amp;amp;b=2"/></outline></body></opml>"#,
        r#"<opml><body></body></opml>"#,
        "not xml at all",
        "",
    ];
    let parsed: Vec<Value> = docs
        .iter()
        .map(|d| match parse_opml(d) {
            Ok(outlines) => json!({ "input": d, "outlines": outlines.iter().map(|o| json!({
                "category": o.category_name,
                "feeds": o.feeds.iter().map(|f| json!({
                    "title": f.title, "xml_url": f.xml_url, "html_url": f.html_url,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>() }),
            Err(e) => json!({ "input": d, "error": e.to_string() }),
        })
        .collect();
    write("opml_parse_cases", &json!(parsed));
}

#[test]
#[ignore = "generator for testdata/vectors"]
fn go_vectors_feeds() {
    let dir = Path::new("testdata/vectors/feeds");
    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "xml"))
        .collect();
    files.sort();
    let fmt = |dt: Option<chrono::DateTime<Utc>>| {
        dt.map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
    };
    for path in files {
        let body = fs::read_to_string(&path).unwrap();
        let parser = feed_rs::parser::Builder::new()
            .timestamp_parser(datetime::parse_timestamp)
            .build();
        let out = match parser.parse(body.as_bytes()) {
            Err(e) => json!({ "error": e.to_string() }),
            Ok(feed) => json!({
                "feed_type": format!("{:?}", feed.feed_type),
                "id": feed.id,
                "title": feed.title.map(|t| t.content),
                "description": feed.description.map(|t| t.content),
                "updated": fmt(feed.updated),
                "links": feed.links.iter().map(|l| &l.href).collect::<Vec<_>>(),
                "icon": feed.icon.map(|i| i.uri),
                "logo": feed.logo.map(|i| i.uri),
                "entries": feed.entries.iter().map(|e| json!({
                    "guid": e.id,
                    "title": e.title.as_ref().map(|t| &t.content),
                    "link": e.links.first().map(|l| &l.href),
                    "links": e.links.iter().map(|l| &l.href).collect::<Vec<_>>(),
                    "content": e.content.as_ref().and_then(|c| c.body.clone())
                        .or_else(|| e.summary.as_ref().map(|s| s.content.clone())),
                    "summary": e.summary.as_ref().map(|s| &s.content),
                    "author": e.authors.first().and_then(|a| a.name.clone()),
                    "published": fmt(e.published),
                    "updated": fmt(e.updated),
                })).collect::<Vec<_>>(),
            }),
        };
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let body = serde_json::to_string_pretty(&out).unwrap() + "\n";
        fs::write(dir.join(format!("{name}.json")), body).unwrap();
    }
}
