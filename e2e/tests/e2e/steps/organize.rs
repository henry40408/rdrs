//! Categories, feeds, OPML import/export, and the flash timestamp.

use std::path::Path;

use anyhow::{Result, ensure};
use cucumber::{given, then, when};
use rdrs_e2e::dom::{Dom, TextContent, submit_element};
use rdrs_e2e::wait::{eventually, eventually_eq};
use rdrs_e2e::world::RdrsWorld;
use thirtyfour::components::SelectElement;
use thirtyfour::prelude::*;

/// Where the committed OPML fixtures live, relative to this crate.
fn fixtures_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures"))
}

// ── Seeding ──────────────────────────────────────────────────────────────────

#[given(expr = "I have a category named {string}")]
async fn have_category(world: &mut RdrsWorld, name: String) -> Result<()> {
    let user_id = world.user_id().await?;
    world
        .seed()
        .create_category(user_id, &name)
        .await
        .map(|_| ())
}

#[given(expr = "the default {string} category is removed")]
async fn default_category_removed(world: &mut RdrsWorld, name: String) -> Result<()> {
    let user_id = world.user_id().await?;
    world.seed().delete_category(user_id, &name).await
}

#[given(expr = "I have a feed {string} in category {string}")]
async fn have_feed_in_category(
    world: &mut RdrsWorld,
    feed_title: String,
    category: String,
) -> Result<()> {
    let username = world.user.username.clone();
    let user_id = world.user_id().await?;
    let seed = world.seed().clone();
    let category_id = seed.create_category(user_id, &category).await?;
    seed.create_feed(
        category_id,
        &format!("https://example.com/{username}-{feed_title}.xml"),
        Some(&feed_title),
    )
    .await
    .map(|_| ())
}

#[given(expr = "I have a feed from the mock RSS server in category {string}")]
async fn have_mock_feed(world: &mut RdrsWorld, category: String) -> Result<()> {
    let feed_url = format!("{}/feed.xml", world.feed_url());
    let user_id = world.user_id().await?;
    let seed = world.seed().clone();
    let category_id = seed.create_category(user_id, &category).await?;
    seed.create_feed(category_id, &feed_url, Some("Test Feed"))
        .await
        .map(|_| ())
}

// ── Pages ────────────────────────────────────────────────────────────────────

#[given("I am on the categories page")]
async fn on_categories_page(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/categories").await?;
    world.driver()?.expect_visible("category-name-input").await
}

/// Waits for the category dropdown, which shows "Loading" until sidebar data lands.
#[given("I am on the feeds page")]
#[when("I am on the feeds page")]
async fn on_feeds_page(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/feeds").await?;
    let driver = world.driver()?;
    eventually("the category dropdown to load", || async {
        let Some(select) = driver.test_id_opt("feed-category-select").await? else {
            return Ok(false);
        };
        Ok(!select.content_text().await?.contains("Loading"))
    })
    .await
}

#[given("I am on the import OPML page")]
async fn on_import_page(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/feeds/import").await?;
    world.driver()?.expect_visible("opml-file-input").await
}

// ── Feeds ────────────────────────────────────────────────────────────────────

#[when(expr = "I add a feed from the mock RSS server under {string}")]
async fn add_mock_feed(world: &mut RdrsWorld, category: String) -> Result<()> {
    let feed_url = format!("{}/feed.xml", world.feed_url());
    let driver = world.driver()?;
    driver.fill("feed-url-input", &feed_url).await?;
    let select = SelectElement::new(&driver.test_id("feed-category-select").await?).await?;
    select.select_by_exact_text(&category).await?;
    driver.submit("add-feed-btn").await
}

#[when(expr = "I refresh the feed {string}")]
async fn refresh_feed(world: &mut RdrsWorld, title: String) -> Result<()> {
    let row = feed_row(world, &title).await?;
    let button = row.find(By::Css("form[action$='/refresh'] button")).await?;
    submit_element(world.driver()?, &button).await
}

#[when(expr = "I edit the feed {string} and set its title to {string}")]
async fn edit_feed_title(
    world: &mut RdrsWorld,
    old_title: String,
    new_title: String,
) -> Result<()> {
    let row = feed_row(world, &old_title).await?;
    let link = row
        .find(By::XPath(".//a[normalize-space(.)='edit']"))
        .await?;
    let driver = world.driver()?;
    submit_element(driver, &link).await?;
    driver.fill("feed-edit-title-input", &new_title).await?;
    driver.submit("feed-edit-save-btn").await
}

/// Arm "I confirm the next dialog" first; the form uses `confirm()`.
#[when(expr = "I delete the feed {string}")]
async fn delete_feed(world: &mut RdrsWorld, title: String) -> Result<()> {
    let row = feed_row(world, &title).await?;
    let button = row.find(By::Css("button.action-link-danger")).await?;
    submit_element(world.driver()?, &button).await
}

/// Labels carry a count suffix, so the option value is looked up first; the
/// select auto-submits.
#[when(expr = "I filter feeds by category {string}")]
async fn filter_feeds_by_category(world: &mut RdrsWorld, name: String) -> Result<()> {
    let driver = world.driver()?;
    let value = driver
        .execute(
            r"
            const sel = document.querySelector('#filter-category');
            const opt = Array.from(sel.options).find(
              (o) => o.text.startsWith(arguments[0] + ' ('),
            );
            return opt ? opt.value : '';
            ",
            vec![serde_json::json!(name)],
        )
        .await?;
    let value = value.json().as_str().unwrap_or_default().to_owned();
    ensure!(
        !value.is_empty(),
        "no category option starts with `{name} (`"
    );

    let select = SelectElement::new(&driver.css("#filter-category").await?).await?;
    select.select_by_value(&value).await?;
    eventually("the feeds list to filter", || async {
        let url = driver.current_url().await?;
        Ok(url.query_pairs().any(|(key, _)| key == "category"))
    })
    .await
}

/// A saved edit lands on the list it was opened from, not the edit page.
#[then("the feeds list is still filtered by category")]
async fn feeds_list_still_filtered(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("the filtered feeds list", || async {
        let url = driver.current_url().await?;
        Ok(url.path() == "/feeds" && url.query_pairs().any(|(key, _)| key == "category"))
    })
    .await
}

#[then(expr = "the feeds table contains {string}")]
async fn feeds_table_contains(world: &mut RdrsWorld, text: String) -> Result<()> {
    world.driver()?.expect_text("feeds-table", &text).await
}

#[then(expr = "the feeds table does not contain {string}")]
async fn feeds_table_lacks(world: &mut RdrsWorld, text: String) -> Result<()> {
    let driver = world.driver()?;
    eventually(&format!("the feeds table to drop {text:?}"), || async {
        let Some(table) = driver.test_id_opt("feeds-table").await? else {
            return Ok(true);
        };
        Ok(!table.content_text().await?.contains(&text))
    })
    .await
}

// ── Categories ───────────────────────────────────────────────────────────────

#[when(expr = "I create a category named {string}")]
async fn create_category(world: &mut RdrsWorld, name: String) -> Result<()> {
    let driver = world.driver()?;
    driver.fill("category-name-input", &name).await?;
    driver.submit("add-category-btn").await
}

/// The `value` attribute keeps the server-rendered name, so the row is scoped
/// before filling.
#[when(expr = "I rename category {string} to {string}")]
async fn rename_category(world: &mut RdrsWorld, old_name: String, new_name: String) -> Result<()> {
    let row = category_row(world, &old_name).await?;
    let field = row.find(By::Css("input[type='text']")).await?;
    field.clear().await?;
    field.send_keys(&new_name).await?;
    let save = row.find(By::Css("button.cat-rename-save")).await?;
    submit_element(world.driver()?, &save).await
}

#[when(expr = "I delete category {string}")]
async fn delete_category(world: &mut RdrsWorld, name: String) -> Result<()> {
    let row = category_row(world, &name).await?;
    let button = row.find(By::Css("button.action-link-danger")).await?;
    submit_element(world.driver()?, &button).await
}

/// Names live in `<input value>`, not DOM text.
#[then(expr = "the categories table contains {string}")]
async fn categories_table_contains(world: &mut RdrsWorld, text: String) -> Result<()> {
    let driver = world.driver()?;
    eventually(&format!("a category named {text:?}"), || async {
        Ok(driver
            .css_opt(&format!(
                r#"[data-testid="categories-table"] input[value="{text}"]"#
            ))
            .await?
            .is_some())
    })
    .await
}

#[then(expr = "the categories table does not contain {string}")]
async fn categories_table_lacks(world: &mut RdrsWorld, text: String) -> Result<()> {
    let driver = world.driver()?;
    eventually(&format!("the category {text:?} to be gone"), || async {
        Ok(driver
            .css_all(&format!(
                r#"[data-testid="categories-table"] input[value="{text}"]"#
            ))
            .await?
            .is_empty())
    })
    .await
}

// ── OPML ─────────────────────────────────────────────────────────────────────

#[when(expr = "I import the OPML fixture {string}")]
async fn import_opml(world: &mut RdrsWorld, filename: String) -> Result<()> {
    let path = fixtures_dir().join(&filename);
    ensure!(path.is_file(), "no OPML fixture at {}", path.display());
    let driver = world.driver()?;
    // Typing an absolute path into the file input; the browser is local.
    driver
        .test_id("opml-file-input")
        .await?
        .send_keys(path.to_string_lossy().as_ref())
        .await?;
    driver.click("opml-import-btn").await
}

/// Fetched with the session cookie; the browser cannot be asked for a download.
#[then(expr = "the exported OPML contains {string}")]
async fn exported_opml_contains(world: &mut RdrsWorld, text: String) -> Result<()> {
    let body = world
        .get_as_user("/reader/api/0/subscription/export")
        .await?;
    ensure!(
        body.contains(&text),
        "the exported OPML does not mention {text:?}"
    );
    Ok(())
}

// ── Flash ────────────────────────────────────────────────────────────────────

#[then(expr = "I see a success flash {string}")]
async fn success_flash(world: &mut RdrsWorld, message: String) -> Result<()> {
    world.driver()?.expect_text("flash-message", &message).await
}

/// Server- and client-rendered flashes must both produce a `<time>` in the
/// viewer's timezone; `rdrs-flash.js` rewrites the server's UTC text.
#[then("the flash banner shows a timestamp")]
async fn flash_shows_timestamp(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    let time = driver.test_id("flash-time").await?;

    // `data-localized` marks the rewrite, so the UTC text is never read mid-swap.
    driver
        .expect_attr(r#"[data-testid="flash-time"]"#, "data-localized", Some(""))
        .await?;

    let text = time.content_text().await?;
    ensure!(
        rdrs_e2e::clock_time_re().is_match(text.trim()),
        "the flash timestamp reads {text:?}, not HH:MM:SS"
    );
    let datetime = time.attr("datetime").await?.unwrap_or_default();
    ensure!(
        !datetime.is_empty(),
        "the flash timestamp carries no datetime"
    );

    let expected = driver
        .execute(
            r"
            return new Date(arguments[0]).toLocaleTimeString(undefined, {
              hour: '2-digit',
              minute: '2-digit',
              second: '2-digit',
              hour12: false,
            });
            ",
            vec![serde_json::json!(datetime)],
        )
        .await?;
    let expected = expected.json().as_str().unwrap_or_default().to_owned();
    eventually_eq("the localised flash timestamp", expected, || async {
        Ok(driver.text_of("flash-time").await?.trim().to_owned())
    })
    .await
}

/// The four row actions (mixed `<a>` and form `<button>`) must share one axis.
/// A block form puts its button on a line box whose strut varies by font
/// (seen on iPadOS Safari only), so the check reruns with an inflated
/// `line-height` to reproduce it in Chromium.
#[then("the actions on a feed row line up on one axis")]
async fn feed_actions_share_an_axis(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    let measured = driver
        .execute(
            r"
            const cell = document.querySelector('.feeds-table tbody tr td.actions');
            if (!cell) return { error: 'no actions cell' };
            // Measure a form's button, not the form.
            const items = [...cell.children].map((el) =>
              el.tagName === 'FORM' ? el.querySelector('button') : el,
            );
            if (items.some((el) => !el)) return { error: 'a form has no button' };

            const measure = () => {
              const rows = items.map((el) => {
                const r = el.getBoundingClientRect();
                return { label: el.textContent.trim(), center: r.top + r.height / 2 };
              });
              const centers = rows.map((r) => r.center);
              return {
                spread: Math.max(...centers) - Math.min(...centers),
                detail: rows.map((r) => `${r.label}@${r.center.toFixed(1)}`).join(' '),
              };
            };

            const rendered = measure();
            // Simulates a taller strut; via CSSOM because CSP refuses injected <style>.
            const original = cell.style.lineHeight;
            cell.style.lineHeight = '3';
            const inflated = measure();
            cell.style.lineHeight = original;

            return { count: items.length, rendered, inflated };
            ",
            vec![],
        )
        .await?;
    let measured = measured.json();

    ensure!(
        measured.get("error").is_none(),
        "could not measure the actions cell: {measured}"
    );
    let count = measured["count"].as_u64().unwrap_or_default();
    ensure!(count == 4, "expected 4 actions, found {count}: {measured}");

    for pass in ["rendered", "inflated"] {
        let spread = measured[pass]["spread"].as_f64().unwrap_or(f64::INFINITY);
        ensure!(
            spread < 1.0,
            "{pass}: the actions are {spread:.1}px apart vertically: {}",
            measured[pass]["detail"]
        );
    }
    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn feed_row(world: &RdrsWorld, title: &str) -> Result<WebElement> {
    row_containing(world, "feeds-table", title).await
}

/// Found by the rename input's `value`.
async fn category_row(world: &RdrsWorld, name: &str) -> Result<WebElement> {
    let selector = format!(r#"[data-testid="categories-table"] tr:has(input[value="{name}"])"#);
    world.driver()?.css(&selector).await
}

async fn row_containing(world: &RdrsWorld, table_id: &str, text: &str) -> Result<WebElement> {
    let table = world.driver()?.test_id(table_id).await?;
    for row in table.find_all(By::Tag("tr")).await? {
        if row.content_text().await.unwrap_or_default().contains(text) {
            return Ok(row);
        }
    }
    anyhow::bail!("`{table_id}` has no row containing {text:?}")
}
