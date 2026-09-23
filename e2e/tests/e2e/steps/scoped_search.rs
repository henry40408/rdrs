//! The entry list's own search drawer.

use anyhow::{Result, ensure};
use cucumber::{given, then, when};
use rdrs_e2e::dom::Dom;
use rdrs_e2e::seed::NewEntry;
use rdrs_e2e::wait::{eventually, eventually_eq};
use rdrs_e2e::world::RdrsWorld;
use thirtyfour::prelude::*;

#[given(expr = "a category {string} containing entries titled {string} and {string}")]
async fn category_with_entries(
    world: &mut RdrsWorld,
    category: String,
    title_a: String,
    title_b: String,
) -> Result<()> {
    let username = world.user.username.clone();
    let user_id = world.user_id().await?;
    let seed = world.seed().clone();
    let category_id = seed.create_category(user_id, &category).await?;
    let feed_id = seed
        .create_feed(
            category_id,
            &format!("https://example.com/{username}-scoped-search.xml"),
            Some(&format!("{category} Feed")),
        )
        .await?;
    let entries = [
        NewEntry::new(feed_id, &format!("{username}-scoped-a"), &title_a)
            .link(format!("https://example.com/{username}/scoped-a"))
            .content(format!("<p>{title_a}</p>"))
            .published_offset("-1 hours"),
        NewEntry::new(feed_id, &format!("{username}-scoped-b"), &title_b)
            .link(format!("https://example.com/{username}/scoped-b"))
            .content(format!("<p>{title_b}</p>"))
            .published_offset("-2 hours"),
    ];
    world.seeded_entries = seed.insert_entries(&entries).await?;
    Ok(())
}

#[when("I open the scoped search box")]
async fn open_scoped_search(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    driver.click("scoped-search-toggle").await?;
    eventually("the scoped search input to take focus", || async {
        driver.is_focused("scoped-search-input").await
    })
    .await
}

#[when("I close the scoped search box")]
async fn close_scoped_search(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.click("scoped-search-close").await
}

#[then("the scoped search box is open")]
async fn scoped_search_open(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    driver.expect_visible("scoped-search-input").await?;
    driver
        .expect_attr(
            r#"[data-testid="scoped-search-toggle"]"#,
            "aria-expanded",
            Some("true"),
        )
        .await
}

/// The collapsed drawer keeps the input in the DOM, just not visible.
#[then("the scoped search box is closed")]
async fn scoped_search_closed(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    driver.expect_hidden("scoped-search-input").await?;
    driver
        .expect_attr(
            r#"[data-testid="scoped-search-toggle"]"#,
            "aria-expanded",
            Some("false"),
        )
        .await
}

/// Auto-submits on a 250 ms debounce; later assertions retry long enough.
#[when(expr = "I type {string} into the scoped search box")]
async fn type_into_scoped_search(world: &mut RdrsWorld, term: String) -> Result<()> {
    world.driver()?.fill("scoped-search-input", &term).await
}

/// Uses backspace: `WebDriver`'s Element Clear dispatches no `input`, so the
/// debounced listener would never run.
#[when("I clear the scoped search box")]
async fn clear_scoped_search(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    let field = driver.test_id("scoped-search-input").await?;
    let value = field.prop("value").await?.unwrap_or_default();
    field.click().await?;
    // Move the caret to the end so backspaces delete everything.
    field.send_keys(Key::End).await?;
    for _ in 0..value.chars().count() {
        field.send_keys(Key::Backspace).await?;
    }
    Ok(())
}

// ── Height parity ────────────────────────────────────────────────────────────

/// Both chips stretch to a sibling's height, which layout changes break silently.
#[then("the search toggle is as tall as the status filter")]
async fn toggle_matches_filter_height(world: &mut RdrsWorld) -> Result<()> {
    expect_same_height(
        world,
        r#"[data-testid="scoped-search-toggle"]"#,
        r#"[data-testid="status-filter-select"]"#,
    )
    .await
}

#[then("the search close button is as tall as the search box")]
async fn close_matches_input_height(world: &mut RdrsWorld) -> Result<()> {
    expect_same_height(
        world,
        r#"[data-testid="scoped-search-close"]"#,
        r#"[data-testid="scoped-search-input"]"#,
    )
    .await
}

/// Polled: the drawer grows over a 0.16s transition, so one measurement
/// depends on machine speed.
async fn expect_same_height(world: &RdrsWorld, one: &str, other: &str) -> Result<()> {
    let driver = world.driver()?;
    let matched = eventually(
        &format!("`{one}` and `{other}` to match in height"),
        || async {
            let (_, _, _, first) = driver.bounding_box(one).await?;
            let (_, _, _, second) = driver.bounding_box(other).await?;
            Ok((first - second).abs() < 1.0)
        },
    )
    .await;
    if matched.is_err() {
        let (_, _, _, first) = driver.bounding_box(one).await?;
        let (_, _, _, second) = driver.bounding_box(other).await?;
        // Report resolved styles, since height alone cannot say why they differ.
        let why = driver
            .execute(
                r"
                const describe = (sel) => {
                  const el = document.querySelector(sel);
                  if (!el) return '(missing)';
                  const s = getComputedStyle(el);
                  return `h=${s.height} min-h=${s.minHeight} pad=${s.paddingTop}/${s.paddingBottom}` +
                         ` font=${s.fontSize} align-self=${s.alignSelf} box=${s.boxSizing}`;
                };
                return {
                  one: describe(arguments[0]),
                  other: describe(arguments[1]),
                  hoverNone: matchMedia('(hover: none)').matches,
                  pointerCoarse: matchMedia('(pointer: coarse)').matches,
                  width: window.innerWidth,
                };
                ",
                vec![serde_json::json!(one), serde_json::json!(other)],
            )
            .await?;
        let why = why.json();
        ensure!(
            (first - second).abs() < 1.0,
            "`{one}` is {first}px tall and `{other}` is {second}px.\n  \
             {one}: {}\n  {other}: {}\n  \
             (hover:none)={} (pointer:coarse)={} innerWidth={}",
            why["one"].as_str().unwrap_or("?"),
            why["other"].as_str().unwrap_or("?"),
            why["hoverNone"],
            why["pointerCoarse"],
            why["width"],
        );
    }
    Ok(())
}

/// On mobile the drawer sits beside the hamburger, and only its row padding
/// keeps their midlines aligned. Polled through the 0.16s expand transition.
#[then("the scoped search box shares its midline with the hamburger")]
async fn drawer_shares_hamburger_midline(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("the drawer and hamburger midlines to agree", || async {
        let (_, input_y, _, input_height) = driver
            .bounding_box(r#"[data-testid="scoped-search-input"]"#)
            .await?;
        let (_, toggle_y, _, toggle_height) = driver.bounding_box(".sidebar-toggle").await?;
        let input_mid = input_y + input_height / 2.0;
        let toggle_mid = toggle_y + toggle_height / 2.0;
        Ok((input_mid - toggle_mid).abs() < 1.0)
    })
    .await
}

// ── Mark-above ───────────────────────────────────────────────────────────────

#[then("the mark-above button is hidden")]
async fn mark_above_hidden(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_absent("mark-above-btn").await
}

#[then("the mark-above button is shown")]
async fn mark_above_shown(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_visible("mark-above-btn").await
}

/// Arms the `confirm` override and clicks in one step; a separate step races
/// the native submit.
#[when("I mark matching entries as read")]
async fn mark_matching_read(world: &mut RdrsWorld) -> Result<()> {
    super::keyboard::accept_next_dialog(world).await?;
    world.driver()?.click("mark-matching-btn").await
}

// ── Results ──────────────────────────────────────────────────────────────────

#[then(expr = "the entry list shows {string}")]
async fn list_shows(world: &mut RdrsWorld, title: String) -> Result<()> {
    eventually(&format!("the list to show {title:?}"), || async {
        Ok(super::entries::entry_row_opt(world, &title)
            .await?
            .is_some())
    })
    .await
}

#[then(expr = "the entry list does not show {string}")]
async fn list_does_not_show(world: &mut RdrsWorld, title: String) -> Result<()> {
    eventually(&format!("the list to drop {title:?}"), || async {
        Ok(super::entries::entry_row_opt(world, &title)
            .await?
            .is_none())
    })
    .await
}

/// Redirects to the same unread `q=` URL, where read entries drop out.
#[then(expr = "{string} is no longer in the unread list")]
async fn no_longer_unread(world: &mut RdrsWorld, title: String) -> Result<()> {
    list_does_not_show(world, title).await
}

// ── The address bar ──────────────────────────────────────────────────────────

/// Polled: `replaceState` runs after the debounced swap. Also used as an `And`
/// barrier after a `When`.
#[then(expr = "the URL has the {string} query parameter set to {string}")]
#[when(expr = "the URL has the {string} query parameter set to {string}")]
async fn url_param_is(world: &mut RdrsWorld, key: String, value: String) -> Result<()> {
    eventually_eq(&format!("the `{key}` query parameter"), Some(value), || {
        query_param(world, &key)
    })
    .await
}

#[then(expr = "the URL has no {string} query parameter")]
async fn url_param_absent(world: &mut RdrsWorld, key: String) -> Result<()> {
    eventually_eq(
        &format!("the `{key}` query parameter"),
        None::<String>,
        || query_param(world, &key),
    )
    .await
}

async fn query_param(world: &RdrsWorld, key: &str) -> Result<Option<String>> {
    let url = world.driver()?.current_url().await?;
    Ok(url
        .query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned()))
}
