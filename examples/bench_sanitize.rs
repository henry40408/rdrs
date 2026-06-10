//! Reproducible micro-benchmark for `sanitize_html`.
//!
//! Run with:
//!   cargo run --release --example bench_sanitize
//!
//! Generates a representative large article (many images + links) and times the
//! full `sanitize_html` pipeline over many iterations. Used to capture
//! before/after numbers for the single-parse refactor.

use std::time::Instant;

use rdrs::services::sanitize::sanitize_html;

const SECRET: &[u8] = b"benchmark_secret_key_32_bytes!!!";

/// Build a representative large article body: paragraphs interleaved with
/// images (some lazy, some tracking pixels) and links (some with tracking
/// params). `blocks` controls the size.
fn make_html(blocks: usize) -> String {
    let mut s = String::with_capacity(blocks * 512);
    s.push_str("<h1>A Representative Article</h1>");
    for i in 0..blocks {
        s.push_str(&format!(
            "<p>Paragraph {i} with some <strong>bold</strong> and <em>italic</em> text, \
             plus an <a href=\"https://example.com/page{i}?utm_source=feed&amp;utm_medium=rss&amp;id={i}\">tracked link</a> \
             and a <a href=\"https://other.example.org/article/{i}\">clean link</a>.</p>"
        ));
        s.push_str(&format!(
            "<figure><img src=\"https://cdn.example.com/img/{i}.jpg?w=1200&amp;h=800\" \
             alt=\"Image {i}\" width=\"1200\" height=\"800\">\
             <figcaption>Caption {i}</figcaption></figure>"
        ));
        // A lazy-loaded image.
        s.push_str(&format!(
            "<img src=\"data:image/svg+xml,%3Csvg%3E%3C/svg%3E\" \
             data-lazy-src=\"https://cdn.example.com/lazy/{i}.jpg\" alt=\"Lazy {i}\">"
        ));
        // A tracking pixel that should be removed.
        s.push_str(&format!(
            "<img src=\"https://pixel.tracker.com/p{i}.gif\" width=\"1\" height=\"1\">"
        ));
    }
    s
}

fn main() {
    let blocks: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(120);
    let iters: usize = std::env::args()
        .nth(2)
        .and_then(|a| a.parse().ok())
        .unwrap_or(200);

    let html = make_html(blocks);
    println!(
        "input size: {} bytes ({} blocks), {} iterations",
        html.len(),
        blocks,
        iters
    );

    // Warm-up.
    for _ in 0..10 {
        let out = sanitize_html(
            &html,
            SECRET,
            Some("https://example.com/post"),
            None,
            Some("https://rdrs.example.com"),
        );
        std::hint::black_box(&out);
    }

    let start = Instant::now();
    for _ in 0..iters {
        let out = sanitize_html(
            &html,
            SECRET,
            Some("https://example.com/post"),
            None,
            Some("https://rdrs.example.com"),
        );
        std::hint::black_box(&out);
    }
    let elapsed = start.elapsed();
    let per = elapsed / iters as u32;
    println!("total: {elapsed:?}");
    println!("per call: {per:?}");
}
