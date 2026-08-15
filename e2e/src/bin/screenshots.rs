//! Regenerates the four screenshots `README.md` embeds.
//!
//!   cd e2e && cargo run --bin screenshots
//!
//! The images are written to `../screenshots/`. Note that a locally generated
//! set will differ from CI's in font rendering and Chromium version, so
//! regenerate them on the machine whose output is being committed — an
//! unrelated image moving in the diff is the tell that the browser changed,
//! not the UI.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use rdrs_e2e::api::{Api, PASSWORD};
use rdrs_e2e::browser::{Browser, Scripting, Viewport};
use rdrs_e2e::dom::{Dom, TextContent};
use rdrs_e2e::seed::{NewEntry, Seed};
use rdrs_e2e::server::{Endpoints, Harness};
use rdrs_e2e::wait::{eventually, eventually_eq};
use thirtyfour::prelude::*;

/// The account the screenshots depict — the instance's administrator, which is
/// what `/api/setup` creates. Going through the invite flow instead would make
/// an ordinary member and quietly drop the admin entries from every captured
/// sidebar.
const DEMO_USER: &str = "demouser";

/// Wide enough for the three-pane desktop layout the README shows.
const VIEWPORT: Viewport = Viewport::new(1920, 1080);

/// How long a favicon fetch may take before the feed goes without one.
const FAVICON_TIMEOUT: Duration = Duration::from_secs(5);

/// Anything larger is not a favicon, and not worth embedding in the fixture.
const FAVICON_MAX_BYTES: usize = 256 * 1024;

/// A feed to seed, its real favicon, and the entries it shows.
struct Feed {
    url: &'static str,
    title: &'static str,
    icon: &'static str,
    entries: &'static [(&'static str, &'static str)],
}

/// Realistic content, inspired by `NetNewsWire`'s default feeds.
const SEED: &[(&str, &[Feed])] = &[
    (
        "Apple & Tech",
        &[
            Feed {
                url: "https://daringfireball.net/feeds/json",
                title: "Daring Fireball",
                icon: "https://daringfireball.net/graphics/favicon-64.png",
                entries: &[
                    (
                        "The M5 Ultra and the Future of Mac Pro",
                        "<p>Apple's latest M5 Ultra chip represents a significant leap in performance for creative professionals. The new Mac Pro, powered by this chip, offers unprecedented capabilities for video editing, 3D rendering, and machine learning workloads. The unified memory architecture now supports up to 512GB, making it a true workstation-class machine.</p>",
                    ),
                    (
                        "Safari 20 Ships With Vertical Tabs",
                        "<p>After years of requests, Safari finally adds vertical tab support. The implementation is clean and native-feeling, with tabs arranged in a sidebar that can be toggled with a keyboard shortcut. It integrates beautifully with Tab Groups and iCloud sync.</p>",
                    ),
                ],
            },
            Feed {
                url: "https://mjtsai.com/blog/feed/",
                title: "Michael Tsai",
                icon: "https://mjtsai.com/favicon.ico",
                entries: &[
                    (
                        "Swift 6.2 Concurrency Changes",
                        "<p>Swift 6.2 brings several refinements to the concurrency model. The most notable change is the introduction of region-based isolation, which makes it easier to reason about data race safety without sacrificing ergonomics.</p>",
                    ),
                    (
                        "App Store Review Times and Transparency",
                        "<p>A collection of developer experiences with recent App Store review processes. Several developers report improved review times, while others note inconsistencies in guideline enforcement across different reviewers.</p>",
                    ),
                ],
            },
            Feed {
                url: "https://sixcolors.com/feed/",
                title: "Six Colors",
                icon: "https://sixcolors.com/favicon.ico",
                entries: &[(
                    "WWDC 2026: What to Expect",
                    "<p>With WWDC just around the corner, here's our comprehensive preview of what Apple might announce. From iOS 20 to macOS 17, visionOS 3, and potential hardware surprises, this year's developer conference promises to be packed with announcements.</p>",
                )],
            },
        ],
    ),
    (
        "Indie & Web",
        &[
            Feed {
                url: "https://inessential.com/feed.json",
                title: "inessential",
                icon: "https://inessential.com/favicon.ico",
                entries: &[
                    (
                        "On Building RSS Readers in 2026",
                        "<p>RSS is having a quiet renaissance. More people are turning to feed readers as an antidote to algorithmic timelines. The protocol's simplicity is its greatest strength — it does one thing well, and it respects the reader's attention.</p>",
                    ),
                    (
                        "Why I Still Write a Blog",
                        "<p>In an era of social media and short-form content, maintaining a blog feels almost rebellious. But there's something deeply satisfying about owning your words, publishing on your own schedule, and building a body of work over decades.</p>",
                    ),
                ],
            },
            Feed {
                url: "https://jvns.ca/atom.xml",
                title: "Julia Evans",
                icon: "https://jvns.ca/favicon.ico",
                entries: &[
                    (
                        "A Little Bit About HTTP Caching",
                        "<p>HTTP caching is one of those things that seems simple on the surface but has a surprising amount of depth. Let's look at Cache-Control headers, ETags, conditional requests, and how browsers actually decide whether to use a cached response.</p>",
                    ),
                    (
                        "How Git Stores Objects",
                        "<p>Ever wondered what's actually inside the .git directory? Let's explore how Git stores commits, trees, and blobs as content-addressed objects, and why this design makes operations like branching and merging so fast.</p>",
                    ),
                ],
            },
            Feed {
                url: "https://kottke.org/feed/json",
                title: "Jason Kottke",
                icon: "https://kottke.org/favicon.ico",
                entries: &[(
                    "The Web We Lost and the Web We Found",
                    "<p>A reflection on how the open web has evolved over the past two decades. While we've lost some of the early web's anarchic creativity, new tools and protocols are enabling a different kind of independence.</p>",
                )],
            },
            Feed {
                url: "https://netnewswire.blog/feed.json",
                title: "NetNewsWire Blog",
                icon: "https://netnewswire.blog/favicon.ico",
                entries: &[(
                    "NetNewsWire 7: What's New",
                    "<p>The latest release of NetNewsWire brings a refreshed design, improved sync performance, and better support for feed discovery. We've also added new keyboard shortcuts and enhanced the reading experience.</p>",
                )],
            },
        ],
    ),
];

#[tokio::main]
async fn main() -> Result<()> {
    let output = screenshot_dir();
    std::fs::create_dir_all(&output).with_context(|| format!("creating {}", output.display()))?;

    let harness = Harness::start().await?;
    let endpoints = harness.endpoints().clone();
    seed_demo_content(&endpoints).await?;

    for theme in ["light", "dark"] {
        capture(&endpoints, theme, &output).await?;
    }
    println!("screenshots: wrote four images to {}", output.display());
    Ok(())
}

/// Where `README.md` looks for the images.
fn screenshot_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("e2e/ always has a parent")
        .join("screenshots")
}

async fn seed_demo_content(endpoints: &Endpoints) -> Result<()> {
    Api::new(&endpoints.base_url)?
        .setup_first_account(DEMO_USER, PASSWORD)
        .await?;
    let seed = Seed::open(&endpoints.db_path).await?;
    let user_id = seed.user_id(DEMO_USER).await?;

    // Fetched in parallel; a feed whose icon cannot be had falls back to the
    // initial chip, which is a legitimate rendering rather than a broken one.
    let icons = fetch_favicons().await;

    let mut hour = 1;
    for (category, feeds) in SEED {
        let category_id = seed.create_category(user_id, category).await?;
        for feed in *feeds {
            let feed_id = seed
                .create_feed(category_id, feed.url, Some(feed.title))
                .await?;
            if let Some((data, content_type)) = icons
                .iter()
                .find(|(url, ..)| *url == feed.icon)
                .map(|(_, data, content_type)| (data.clone(), content_type.clone()))
            {
                seed.insert_icon(feed_id, &data, &content_type, Some(feed.icon))
                    .await?;
            }

            let entries: Vec<_> = feed
                .entries
                .iter()
                .enumerate()
                .map(|(index, (title, content))| {
                    let origin = feed
                        .url
                        .split_once("/feed")
                        .map_or(feed.url, |(head, _)| head);
                    let slug: String = title
                        .to_lowercase()
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                        .collect();
                    let entry =
                        NewEntry::new(feed_id, &format!("{}/entry-{index}", feed.url), title)
                            .link(format!("{origin}/{slug}"))
                            .content(*content)
                            .published_offset(format!("-{hour} hours"));
                    hour += 1;
                    entry
                })
                .collect();
            seed.insert_entries(&entries).await?;
        }
    }
    Ok(())
}

/// Fetches every favicon, returning the ones that arrived.
async fn fetch_favicons() -> Vec<(&'static str, Vec<u8>, String)> {
    let client = reqwest::Client::builder()
        .timeout(FAVICON_TIMEOUT)
        .build()
        .expect("the favicon client builds");
    let mut fetches = Vec::new();
    for (_, feeds) in SEED {
        for feed in *feeds {
            let client = client.clone();
            fetches.push(async move {
                let response = client.get(feed.icon).send().await.ok()?;
                if !response.status().is_success() {
                    return None;
                }
                let content_type = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("image/x-icon")
                    .to_owned();
                let data = response.bytes().await.ok()?.to_vec();
                if data.is_empty() || data.len() > FAVICON_MAX_BYTES {
                    return None;
                }
                Some((feed.icon, data, content_type))
            });
        }
    }
    futures_util::future::join_all(fetches)
        .await
        .into_iter()
        .flatten()
        .collect()
}

/// Signs in and captures both images for one colour scheme.
async fn capture(endpoints: &Endpoints, theme: &str, output: &Path) -> Result<()> {
    let mut browser = Browser::open(Scripting::Enabled).await?;
    browser.set_viewport(VIEWPORT).await?;
    // Emulated rather than stored as a preference: this is the app's
    // system-follow path, which is what the screenshots are meant to show.
    browser.emulate_color_scheme(theme).await?;
    let driver = browser.driver();
    let base = &endpoints.base_url;
    let suffix = if theme == "dark" { "-dark" } else { "" };

    driver.goto(format!("{base}/login")).await?;
    driver.fill("username-input", DEMO_USER).await?;
    driver.fill("password-input", PASSWORD).await?;
    driver.submit("login-submit").await?;
    driver.expect_visible("entry-item").await?;

    // Every feed icon must have finished loading — or failed — before a
    // capture, or the image catches a half-drawn sidebar.
    eventually("the feed icons to settle", || async {
        let done = driver
            .eval(
                "return [...document.querySelectorAll('.feed-icon')]\
                 .every((img) => img.complete);",
            )
            .await?;
        Ok(done.as_bool().unwrap_or(false))
    })
    .await?;

    // `j` moves the list cursor to the first row; `o` opens it in the reading
    // pane (j/k navigate the pane only once it is open).
    let before: i64 = driver
        .css("#unread-count")
        .await?
        .content_text()
        .await?
        .trim()
        .parse()
        .unwrap_or(0);
    driver.press("j").await?;
    driver.press_focused("o").await?;
    driver.css(".reading-pane-title").await?;
    // The pane swap and the sidebar count are two independent round trips:
    // opening marks the entry read, and the new count arrives over SSE and then
    // through `<rdrs-sidebar>`'s coalescing debounce. Waiting only on the pane
    // captures the row already showing its read dot while the badge still shows
    // the pre-read total — an internally inconsistent screenshot.
    eventually_eq("the unread badge", (before - 1).to_string(), || async {
        Ok(driver
            .css("#unread-count")
            .await?
            .content_text()
            .await?
            .trim()
            .to_owned())
    })
    .await?;
    write_png(driver, &output.join(format!("unread-list{suffix}.png"))).await?;

    driver.press("?").await?;
    eventually("the help overlay to open", || async {
        let Some(overlay) = driver.css_opt("rdrs-kb-help").await? else {
            return Ok(false);
        };
        Ok(overlay
            .class_name()
            .await?
            .unwrap_or_default()
            .split_whitespace()
            .any(|name| name == "visible"))
    })
    .await?;
    // The overlay fades in; capturing on the class alone catches it mid-fade.
    tokio::time::sleep(Duration::from_millis(200)).await;
    write_png(
        driver,
        &output.join(format!("keyboard-shortcuts{suffix}.png")),
    )
    .await?;

    browser.quit().await?;
    Ok(())
}

async fn write_png(driver: &WebDriver, path: &Path) -> Result<()> {
    let png = driver.screenshot_as_png().await?;
    std::fs::write(path, png).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
