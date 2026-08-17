//! Seeding entries, navigating the lists, and asserting on the rows.
//!
//! One of four modules covering `entries.steps.js`, which mixed the entry list,
//! the reading pane, the sidebar and the keyboard shortcuts in one 1,000-line
//! file. See also [`super::reading_pane`], [`super::sidebar`] and
//! [`super::keyboard`].

use anyhow::{Result, ensure};
use cucumber::{given, then, when};
use rdrs_e2e::dom::{Dom, TextContent, Within};
use rdrs_e2e::wait::{eventually, eventually_eq, eventually_some};
use rdrs_e2e::world::RdrsWorld;

/// A 1×1 transparent PNG — enough for `feed_has_icon` to render an `<img>`
/// favicon rather than the initial chip.
const TRANSPARENT_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00, 0x00, 0xb5, 0x1c, 0x0c,
    0x02, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8, 0xcf, 0x00, 0x00,
    0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44,
    0xae, 0x42, 0x60, 0x82,
];

// ── Seeding ──────────────────────────────────────────────────────────────────

#[given(expr = "I have a feed {string} with {int} test entries in category {string}")]
async fn feed_with_entries_in_category(
    world: &mut RdrsWorld,
    feed_title: String,
    count: u32,
    category: String,
) -> Result<()> {
    let username = world.user.username.clone();
    let user_id = world.user_id().await?;
    let seed = world.seed().clone();
    let category_id = seed.create_category(user_id, &category).await?;
    let feed_id = seed
        .create_feed(
            category_id,
            &format!("https://example.com/{username}-{feed_title}.xml"),
            Some(&feed_title),
        )
        .await?;
    world.seeded_entries = seed.seed_test_entries(feed_id, count).await?;
    Ok(())
}

#[given(expr = "the {string} feed has a favicon")]
async fn feed_has_favicon(world: &mut RdrsWorld, feed_title: String) -> Result<()> {
    let user_id = world.user_id().await?;
    let seed = world.seed().clone();
    let feed_id = seed.feed_id_by_title(user_id, &feed_title).await?;
    seed.insert_icon(
        feed_id,
        TRANSPARENT_PNG,
        "image/png",
        Some("https://example.com/icon.png"),
    )
    .await
}

/// Points an entry at a link the readability fetcher rejects outright. The SSRF
/// guard in `utils/url_validation.rs` blocks loopback before any network I/O,
/// so Fetch Full Content answers immediately with its error flash instead of
/// waiting on DNS — which is what a scenario about the *round trip* needs, and
/// what the seeded `https://example.com` links cannot give: they resolve (or
/// hang) depending on whether the machine has internet, and the fetch fails
/// either way once the extractor sees a 404.
#[given(expr = "the entry titled {string} cannot have its full content fetched")]
async fn entry_cannot_fetch_full_content(world: &mut RdrsWorld, title: String) -> Result<()> {
    let entry_id = entry_id(world, &title).await?;
    world
        .seed()
        .set_entry_link(entry_id, "http://127.0.0.1/blocked")
        .await
}

#[given(expr = "the entry titled {string} is marked read")]
async fn entry_marked_read(world: &mut RdrsWorld, title: String) -> Result<()> {
    let entry_id = entry_id(world, &title).await?;
    world.seed().mark_read(entry_id, "0 seconds").await
}

/// Backdated so the read lands strictly *before* the page's render-time
/// snapshot — a `datetime('now')` read in the same second as the render would
/// fall inside the `>=` snapshot boundary and make skip-assertions flaky.
#[given(expr = "the entry titled {string} was marked read an hour ago")]
async fn entry_read_an_hour_ago(world: &mut RdrsWorld, title: String) -> Result<()> {
    let entry_id = entry_id(world, &title).await?;
    world.seed().mark_read(entry_id, "-1 hour").await
}

#[given(expr = "the entry titled {string} is starred")]
async fn entry_starred(world: &mut RdrsWorld, title: String) -> Result<()> {
    let entry_id = entry_id(world, &title).await?;
    world.seed().mark_starred(entry_id, "0 seconds").await
}

#[given(expr = "the entry titled {string} has a summary")]
async fn entry_has_summary(world: &mut RdrsWorld, title: String) -> Result<()> {
    let user_id = world.user_id().await?;
    let entry_id = entry_id(world, &title).await?;
    world
        .seed()
        .insert_summary(entry_id, user_id, "summary.")
        .await
}

#[given(expr = "the entry titled {string} has a failed summary")]
async fn entry_has_failed_summary(world: &mut RdrsWorld, title: String) -> Result<()> {
    let user_id = world.user_id().await?;
    let entry_id = entry_id(world, &title).await?;
    world
        .seed()
        .insert_failed_summary(entry_id, user_id, "Kagi API returned 503.")
        .await
}

#[given(expr = "the entry titled {string} has a pending summary")]
async fn entry_has_pending_summary(world: &mut RdrsWorld, title: String) -> Result<()> {
    let user_id = world.user_id().await?;
    let entry_id = entry_id(world, &title).await?;
    world.seed().insert_pending_summary(entry_id, user_id).await
}

#[given(expr = "the entry titled {string} has content with a broken image")]
async fn entry_has_broken_image(world: &mut RdrsWorld, title: String) -> Result<()> {
    let entry_id = entry_id(world, &title).await?;
    world
        .seed()
        .set_entry_content(
            entry_id,
            r#"<p>x</p><img src="https://images.internal/missing.jpg" alt="Missing diagram">"#,
        )
        .await
}

/// Mirrors Rouge's line-numbered output: an outer `<pre>` wrapping a `<code>`
/// plus a `<table>` whose cells each hold their own nested `<pre>` (gutter and
/// code).
#[given(expr = "the entry titled {string} contains a line-numbered code block")]
async fn entry_has_code_block(world: &mut RdrsWorld, title: String) -> Result<()> {
    let entry_id = entry_id(world, &title).await?;
    let html = concat!(
        r#"<div class="highlight"><pre class="highlight"><code>"#,
        r#"<table class="rouge-table"><tbody><tr>"#,
        "<td class=\"rouge-gutter\"><pre class=\"lineno\">1\n2\n3\n</pre></td>",
        "<td class=\"rouge-code\"><pre>line one\nline two\nline three\n</pre></td>",
        "</tr></tbody></table></code></pre></div>",
    );
    world.seed().set_entry_content(entry_id, html).await
}

#[given(expr = "all entries in category {string} are marked read")]
async fn category_all_read(world: &mut RdrsWorld, category: String) -> Result<()> {
    let user_id = world.user_id().await?;
    world.seed().mark_category_read(user_id, &category).await
}

#[given(expr = "the feed has {int} entries")]
async fn feed_has_entries(world: &mut RdrsWorld, count: u32) -> Result<()> {
    let user_id = world.user_id().await?;
    let seed = world.seed().clone();
    let feed_id = seed.first_feed_id(user_id).await?;
    world.seeded_entries = seed.seed_test_entries(feed_id, count).await?;
    Ok(())
}

#[given(expr = "I have {int} more categories")]
async fn more_categories(world: &mut RdrsWorld, count: u32) -> Result<()> {
    let user_id = world.user_id().await?;
    let seed = world.seed().clone();
    for i in 1..=count {
        seed.create_category(user_id, &format!("Filler {i}"))
            .await?;
    }
    Ok(())
}

/// The same filler, but with something unread in each one, for the scenarios
/// that also turn on the hide-fully-read setting: it drops every empty
/// category, and a sidebar short enough to fit has no scroll offset to lose.
#[given(expr = "I have {int} more categories with unread entries")]
async fn more_categories_with_unread(world: &mut RdrsWorld, count: u32) -> Result<()> {
    let username = world.user.username.clone();
    let user_id = world.user_id().await?;
    let seed = world.seed().clone();
    for i in 1..=count {
        let category_id = seed
            .create_category(user_id, &format!("Filler {i}"))
            .await?;
        let feed_id = seed
            .create_feed(
                category_id,
                &format!("https://example.com/{username}-filler-{i}.xml"),
                Some(&format!("Filler Feed {i}")),
            )
            .await?;
        seed.seed_test_entries(feed_id, 1).await?;
    }
    Ok(())
}

// ── Navigation ───────────────────────────────────────────────────────────────

#[when("I open the all entries page")]
async fn open_all_entries(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/entries").await
}

#[when("I open the read entries page")]
async fn open_read_entries(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/entries/read").await
}

#[when("I open the starred entries page")]
async fn open_starred_entries(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/entries/starred").await
}

#[when("I open the summarized entries page")]
async fn open_summarized_entries(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/entries/summarized").await
}

#[when(expr = "I open the entries page for feed {string}")]
async fn open_feed_entries(world: &mut RdrsWorld, feed_title: String) -> Result<()> {
    let feed_id = feed_id(world, &feed_title).await?;
    world.goto(&format!("/feeds/{feed_id}/entries")).await
}

#[when(expr = "I open the entries page for category {string}")]
async fn open_category_entries(world: &mut RdrsWorld, category: String) -> Result<()> {
    let category_id = category_id(world, &category).await?;
    world
        .goto(&format!("/categories/{category_id}/entries"))
        .await
}

/// `?status=all` keeps read entries listed, which is what the morph scenarios
/// need: on the unread view a row that is marked read simply leaves, and a row
/// that is gone proves nothing about whether the ones that stayed were rebuilt.
#[when(expr = "I open the entries page for category {string} showing all statuses")]
async fn open_category_all_statuses(world: &mut RdrsWorld, category: String) -> Result<()> {
    let category_id = category_id(world, &category).await?;
    world
        .goto(&format!("/categories/{category_id}/entries?status=all"))
        .await
}

#[when(expr = "I open the entries page for category {string} searching for {string}")]
async fn open_category_search(
    world: &mut RdrsWorld,
    category: String,
    query: String,
) -> Result<()> {
    let category_id = category_id(world, &category).await?;
    let encoded: String = url_encode(&query);
    world
        .goto(&format!("/categories/{category_id}/entries?q={encoded}"))
        .await
}

#[when(expr = "I open the inbox deep-linked to entry titled {string}")]
async fn open_inbox_deep_linked(world: &mut RdrsWorld, title: String) -> Result<()> {
    let entry_id = entry_id(world, &title).await?;
    world.goto(&format!("/?entry={entry_id}")).await
}

#[when("I reload the page")]
async fn reload(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.refresh().await?;
    Ok(())
}

#[when("I go back in the browser")]
async fn go_back(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.back().await?;
    Ok(())
}

// ── Where the browser ended up ───────────────────────────────────────────────

#[then("I am on the unread inbox")]
async fn on_unread_inbox(world: &mut RdrsWorld) -> Result<()> {
    world.expect_path("/").await
}

#[then("I am on the all entries page")]
async fn on_all_entries(world: &mut RdrsWorld) -> Result<()> {
    world.expect_path("/entries").await
}

#[then("I am on the starred entries page")]
async fn on_starred_entries(world: &mut RdrsWorld) -> Result<()> {
    world.expect_path("/entries/starred").await
}

#[then(expr = "I am on the entries page for feed {string}")]
async fn on_feed_entries(world: &mut RdrsWorld, feed_title: String) -> Result<()> {
    let feed_id = feed_id(world, &feed_title).await?;
    world
        .expect_path(&format!("/feeds/{feed_id}/entries"))
        .await
}

#[then(expr = "I am on the Read filter for feed {string}")]
async fn on_feed_read_filter(world: &mut RdrsWorld, feed_title: String) -> Result<()> {
    let feed_id = feed_id(world, &feed_title).await?;
    world
        .expect_path(&format!("/feeds/{feed_id}/entries?status=read"))
        .await
}

// Also reachable as an `And` following a `When`, which cucumber resolves to
// `when` — the scenario uses it as a barrier before the next interaction.
#[then(expr = "I am on the entries page for category {string}")]
#[when(expr = "I am on the entries page for category {string}")]
async fn on_category_entries(world: &mut RdrsWorld, category: String) -> Result<()> {
    let category_id = category_id(world, &category).await?;
    world
        .expect_path(&format!("/categories/{category_id}/entries"))
        .await
}

#[then(expr = "the browser is on {string}")]
async fn browser_is_on(world: &mut RdrsWorld, path: String) -> Result<()> {
    let driver = world.driver()?;
    eventually(&format!("the URL to end in {path:?}"), || async {
        Ok(driver.current_url().await?.path().ends_with(&path))
    })
    .await
}

#[when(expr = "I click the breadcrumb link {string}")]
async fn click_breadcrumb(world: &mut RdrsWorld, label: String) -> Result<()> {
    let breadcrumb = world.driver()?.test_id("breadcrumb").await?;
    let link = breadcrumb
        .link_named(&label)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the breadcrumb has no `{label}` link"))?;
    link.click().await?;
    Ok(())
}

// ── Row assertions ───────────────────────────────────────────────────────────

#[then(expr = "I see {int} entries in the entry list")]
#[then(expr = "I see {int} entry in the entry list")]
async fn see_n_entries(world: &mut RdrsWorld, count: usize) -> Result<()> {
    let driver = world.driver()?;
    eventually_eq("the number of entry rows", count, || async {
        Ok(driver.test_ids("entry-item").await?.len())
    })
    .await
}

/// Polls, so an async swap (a Load More fetch) has a chance to land — a plain
/// count snapshots the DOM at one instant.
#[then(expr = "I see more than {int} entries in the entry list")]
async fn see_more_than(world: &mut RdrsWorld, count: usize) -> Result<()> {
    let driver = world.driver()?;
    eventually(&format!("more than {count} entry rows"), || async {
        Ok(driver.test_ids("entry-item").await?.len() > count)
    })
    .await
}

#[then(expr = "the first entry is titled {string}")]
async fn first_entry_titled(world: &mut RdrsWorld, title: String) -> Result<()> {
    let driver = world.driver()?;
    eventually(&format!("the first entry is {title:?}"), || async {
        Ok(driver
            .text_of_test_id("entry-item")
            .await?
            .is_some_and(|text| text.contains(&title)))
    })
    .await
}

#[then(expr = "the entry row for {string} shows as read")]
async fn row_shows_read(world: &mut RdrsWorld, title: String) -> Result<()> {
    expect_row_class(world, &title, "entry-read", true).await
}

#[then(expr = "the entry row for {string} shows as unread")]
async fn row_shows_unread(world: &mut RdrsWorld, title: String) -> Result<()> {
    expect_row_class(world, &title, "entry-read", false).await
}

/// The starred state lives on the star-action toggle — when starred it shows ★
/// and flips to `aria-label="Unstar"` plus a POST to `/unstar`.
#[then(expr = "the entry row for {string} shows as starred")]
async fn row_shows_starred(world: &mut RdrsWorld, title: String) -> Result<()> {
    eventually(&format!("`{title}` is starred"), || async {
        let Some(row) = entry_row_opt(world, &title).await? else {
            return Ok(false);
        };
        let Some(action) = row.test_id_opt("entry-star-action").await? else {
            return Ok(false);
        };
        Ok(action.attr("aria-label").await?.as_deref() == Some("Unstar"))
    })
    .await
}

#[then("every entry in the list is marked read")]
async fn every_entry_read(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("every row carries `entry-read`", || async {
        let rows = driver
            .css_all("[data-entries-list] [data-entry-row]")
            .await?;
        if rows.is_empty() {
            return Ok(false);
        }
        for row in rows {
            if !row
                .class_name()
                .await?
                .unwrap_or_default()
                .contains("entry-read")
            {
                return Ok(false);
            }
        }
        Ok(true)
    })
    .await
}

#[then(expr = "the list header shows {string}")]
async fn list_header_shows(world: &mut RdrsWorld, title: String) -> Result<()> {
    let driver = world.driver()?;
    eventually(&format!("the list header shows {title:?}"), || async {
        Ok(driver
            .text_of_css(".list-pane-header h1")
            .await?
            .is_some_and(|text| text.contains(&title)))
    })
    .await
}

#[then("I see no flash message")]
async fn no_flash(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_absent("flash-message").await
}

/// A barrier for form-action swaps whose only visible signal is a toast (Save,
/// Fetch Full Content). The flash is shown right after the reading-pane swap
/// lands and the neighbour re-resolve fires, so waiting on it sequences any
/// follow-up navigation after the pane has fully settled — which is why the
/// scenarios reach it as a `When` as often as a `Then`.
#[then("I see a flash message")]
#[when("I see a flash message")]
async fn see_flash(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_visible("flash-message").await
}

/// The feed-title `<a>` must shrink-wrap its text. A full-width block link
/// makes clicks on the blank space after a short feed name navigate to the feed
/// (`installRowClickToOpen` defers to any anchor under the pointer) instead of
/// falling through to the row's open-entry handler.
#[then("the feed link does not span the full meta row")]
async fn feed_link_shrink_wraps(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    let (_, _, link_width, _) = driver
        .bounding_box("[data-testid=\"entry-item\"] .entry-feed")
        .await?;
    let (_, _, container_width, _) = driver
        .bounding_box("[data-testid=\"entry-item\"] .entry-meta-text")
        .await?;
    ensure!(
        link_width < container_width,
        "the feed link is {link_width}px wide inside a {container_width}px row, so it spans it"
    );
    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// The entry row whose text contains `title`, if the list has one.
pub async fn entry_row_opt(
    world: &RdrsWorld,
    title: &str,
) -> Result<Option<thirtyfour::WebElement>> {
    for row in world.driver()?.test_ids("entry-item").await? {
        if row.content_text().await.unwrap_or_default().contains(title) {
            return Ok(Some(row));
        }
    }
    Ok(None)
}

/// The entry row whose text contains `title`, waiting for it to arrive.
pub async fn entry_row(world: &RdrsWorld, title: &str) -> Result<thirtyfour::WebElement> {
    eventually_some(&format!("an entry row containing {title:?}"), || {
        entry_row_opt(world, title)
    })
    .await
}

async fn expect_row_class(
    world: &RdrsWorld,
    title: &str,
    class: &str,
    present: bool,
) -> Result<()> {
    eventually(
        &format!(
            "`{title}` {} `{class}`",
            if present { "has" } else { "lacks" }
        ),
        || async {
            let Some(row) = entry_row_opt(world, title).await? else {
                return Ok(false);
            };
            let classes = row.class_name().await?.unwrap_or_default();
            Ok(classes.split_whitespace().any(|name| name == class) == present)
        },
    )
    .await
}

pub async fn entry_id(world: &mut RdrsWorld, title: &str) -> Result<i64> {
    let user_id = world.user_id().await?;
    world.seed().entry_id_by_title(user_id, title).await
}

pub async fn feed_id(world: &mut RdrsWorld, feed_title: &str) -> Result<i64> {
    let user_id = world.user_id().await?;
    world.seed().feed_id_by_title(user_id, feed_title).await
}

pub async fn category_id(world: &mut RdrsWorld, category: &str) -> Result<i64> {
    let user_id = world.user_id().await?;
    world.seed().category_id(user_id, category).await
}

/// Percent-encodes a query value, `encodeURIComponent`'s job.
fn url_encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}
