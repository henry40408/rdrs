//! Sidebar category/feed rows, badges, and in-place reconciliation.

use anyhow::{Result, ensure};
use cucumber::{then, when};
use rdrs_e2e::dom::{Dom, TextContent};
use rdrs_e2e::wait::{eventually, eventually_eq, eventually_some};
use rdrs_e2e::world::RdrsWorld;
use thirtyfour::prelude::*;

const CATEGORY_LINK: &str = "#sidebar-categories a[data-category-id]";
const FEED_LINK: &str = ".sidebar-feed[data-feed-id]";

/// The category shortcuts no-op on an empty category list, so a keypress
/// racing the sidebar fetch would fail for unrelated reasons.
#[when("the sidebar has loaded its categories")]
async fn sidebar_loaded(world: &mut RdrsWorld) -> Result<()> {
    world
        .driver()?
        .css("#sidebar-categories a")
        .await
        .map(|_| ())
}

#[when(expr = "I click the sidebar category {string}")]
async fn click_category(world: &mut RdrsWorld, name: String) -> Result<()> {
    category_link(world, &name).await?.click().await?;
    Ok(())
}

#[when(expr = "I click the sidebar feed {string}")]
async fn click_feed(world: &mut RdrsWorld, title: String) -> Result<()> {
    feed_link(world, &title).await?.click().await?;
    Ok(())
}

#[then(expr = "the sidebar highlights category {string}")]
async fn highlights_category(world: &mut RdrsWorld, name: String) -> Result<()> {
    eventually(&format!("category `{name}` is active"), || async {
        let Some(link) = category_link_opt(world, &name).await? else {
            return Ok(false);
        };
        has_class(&link, "active").await
    })
    .await
}

// Also used as an `And` after a `When` to wait for the sidebar.
#[then(expr = "the sidebar lists feed {string}")]
#[when(expr = "the sidebar lists feed {string}")]
async fn lists_feed(world: &mut RdrsWorld, title: String) -> Result<()> {
    let link = feed_link(world, &title).await?;
    ensure!(
        link.is_displayed().await?,
        "feed `{title}` is present but hidden"
    );
    Ok(())
}

#[then(expr = "the sidebar does not list feed {string}")]
async fn does_not_list_feed(world: &mut RdrsWorld, title: String) -> Result<()> {
    eventually(&format!("feed `{title}` is gone"), || async {
        Ok(feed_link_opt(world, &title).await?.is_none())
    })
    .await
}

#[then(expr = "the sidebar highlights feed {string}")]
async fn highlights_feed(world: &mut RdrsWorld, title: String) -> Result<()> {
    eventually(&format!("feed `{title}` is active"), || async {
        let Some(link) = feed_link_opt(world, &title).await? else {
            return Ok(false);
        };
        has_class(&link, "active").await
    })
    .await
}

#[then(expr = "the sidebar feed {string} shows {int} unread")]
async fn feed_unread(world: &mut RdrsWorld, title: String, count: u32) -> Result<()> {
    eventually_eq(
        &format!("feed `{title}`'s unread badge"),
        count.to_string(),
        || async {
            let link = feed_link(world, &title).await?;
            let badge = link.find(By::Css(".sidebar-badge")).await?;
            Ok(badge.content_text().await?.trim().to_owned())
        },
    )
    .await
}

#[then(expr = "the sidebar feed {string} shows its icon")]
async fn feed_shows_icon(world: &mut RdrsWorld, title: String) -> Result<()> {
    let link = feed_link(world, &title).await?;
    let icon = link
        .find(By::Css("img.entry-favicon"))
        .await
        .map_err(|_| anyhow::anyhow!("feed `{title}` renders no image favicon"))?;
    ensure!(
        icon.is_displayed().await?,
        "feed `{title}`'s icon is hidden"
    );
    let src = icon.attr("src").await?.unwrap_or_default();
    ensure!(
        rdrs_e2e::feed_icon_src_re().is_match(&src),
        "feed `{title}`'s icon points at {src:?}, not the icon endpoint"
    );
    Ok(())
}

/// No-icon fallback: the first letter, uppercased.
#[then(expr = "the sidebar feed {string} shows an initial chip")]
async fn feed_shows_chip(world: &mut RdrsWorld, title: String) -> Result<()> {
    let expected: String = title.chars().take(1).flat_map(char::to_uppercase).collect();
    eventually_eq(
        &format!("feed `{title}`'s initial chip"),
        expected,
        || async {
            let link = feed_link(world, &title).await?;
            let chip = link.find(By::Css(".entry-favicon-chip")).await?;
            Ok(chip.content_text().await?.trim().to_owned())
        },
    )
    .await
}

/// A re-rendered row has no `data-e2e-tag`, so a surviving tag proves the row
/// (and favicon) was patched in place; a rebuilt `<img>` blanks a frame in `WebKit`.
#[when("I tag the sidebar feed rows")]
async fn tag_feed_rows(world: &mut RdrsWorld) -> Result<()> {
    let tagged = world
        .driver()?
        .eval(
            r#"
            const rows = [...document.querySelectorAll(".sidebar-feed[data-feed-id]")];
            for (const row of rows) {
              row.dataset.e2eTag = "1";
              const icon = row.querySelector(".entry-favicon");
              if (icon) icon.dataset.e2eTag = "1";
            }
            return rows.length;
            "#,
        )
        .await?;
    ensure!(
        tagged.as_u64().unwrap_or(0) > 0,
        "there were no sidebar feed rows to tag"
    );
    Ok(())
}

#[then("the sidebar feed rows are still the ones I tagged")]
async fn feed_rows_still_tagged(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    for selector in [
        ".sidebar-feed[data-feed-id]:not([data-e2e-tag])",
        ".sidebar-feed[data-feed-id] .entry-favicon:not([data-e2e-tag])",
    ] {
        let rebuilt = driver.css_all(selector).await?.len();
        ensure!(
            rebuilt == 0,
            "{rebuilt} node(s) matching `{selector}` were rebuilt"
        );
    }
    Ok(())
}

/// `<rdrs-sidebar>` re-fetches `/api/sidebar` after mount; wait until `name`'s
/// badge is gone so the freshest counts are on screen.
#[when(expr = "the sidebar shows no unread for category {string}")]
async fn category_has_no_unread(world: &mut RdrsWorld, name: String) -> Result<()> {
    eventually(
        &format!("category `{name}` has no unread badge"),
        || async {
            let driver = world.driver()?;
            for link in driver
                .css_all(r#"rdrs-sidebar a[href^="/categories/"]"#)
                .await?
            {
                if !link
                    .content_text()
                    .await
                    .unwrap_or_default()
                    .contains(&name)
                {
                    continue;
                }
                return Ok(link
                    .query(By::Css(".sidebar-badge"))
                    .nowait()
                    .first_opt()
                    .await?
                    .is_none());
            }
            // Link gone means no badge either.
            Ok(true)
        },
    )
    .await
}

#[then("the sidebar highlights All Entries")]
async fn highlights_all_entries(world: &mut RdrsWorld) -> Result<()> {
    expect_testid_active(world, "nav-entries").await
}

#[then("the sidebar highlights Summarized")]
async fn highlights_summarized(world: &mut RdrsWorld) -> Result<()> {
    expect_testid_active(world, "nav-summarized").await
}

/// The Starred item has no `data-testid`; address it by `href`.
#[then("the sidebar highlights Starred")]
async fn highlights_starred(world: &mut RdrsWorld) -> Result<()> {
    eventually("the Starred item is active", || async {
        let Some(link) = world
            .driver()?
            .css_opt(r#"rdrs-sidebar a[href="/entries/starred"]"#)
            .await?
        else {
            return Ok(false);
        };
        has_class(&link, "active").await
    })
    .await
}

#[then(expr = "the sidebar Summarized item shows a count of {string}")]
async fn summarized_count(world: &mut RdrsWorld, count: String) -> Result<()> {
    let driver = world.driver()?;
    eventually_eq("the Summarized count", count, || async {
        Ok(driver
            .css(r#"[data-testid="nav-summarized"] #summarized-count"#)
            .await?
            .content_text()
            .await?
            .trim()
            .to_owned())
    })
    .await
}

/// Regression: a reload or `innerHTML` re-render reset `.sidebar-nav` scroll to
/// the top, hiding the just-clicked category.
#[when("I scroll the sidebar categories to the bottom")]
async fn scroll_sidebar_bottom(world: &mut RdrsWorld) -> Result<()> {
    let offset = world
        .driver()?
        .eval(
            r"
            const nav = document.querySelector('.sidebar-nav');
            nav.scrollTop = nav.scrollHeight;
            window.__rdrsSidebarScroll = nav.scrollTop;
            return nav.scrollTop;
            ",
        )
        .await?;
    ensure!(
        offset.as_f64().unwrap_or(0.0) > 0.0,
        "the sidebar nav must actually overflow for this scenario"
    );
    Ok(())
}

/// The *last* category, so the click's scroll-into-view does not move
/// `.sidebar-nav` and confound the assertion.
#[when("I click the last sidebar category")]
async fn click_last_category(world: &mut RdrsWorld) -> Result<()> {
    let links = world.driver()?.css_all(CATEGORY_LINK).await?;
    let link = links
        .last()
        .ok_or_else(|| anyhow::anyhow!("the sidebar lists no categories"))?;
    let name = link
        .find(By::Css(".sidebar-item-label"))
        .await?
        .content_text()
        .await?
        .trim()
        .to_owned();
    link.click().await?;

    let driver = world.driver()?;
    eventually(&format!("the list header shows {name:?}"), || async {
        Ok(driver
            .text_of_css(".list-pane-header h1")
            .await?
            .is_some_and(|text| text.contains(&name)))
    })
    .await
}

#[then("the sidebar is still scrolled where it was")]
async fn sidebar_still_scrolled(world: &mut RdrsWorld) -> Result<()> {
    let probe = world
        .driver()?
        .eval(
            "return [window.__rdrsSidebarScroll, document.querySelector('.sidebar-nav').scrollTop];",
        )
        .await?;
    let noted = probe[0].as_f64().unwrap_or(0.0);
    let now = probe[1].as_f64().unwrap_or(0.0);
    ensure!(
        noted > 0.0,
        "the noted offset is gone — the document reloaded"
    );
    // Not exact: the open category's feed list changes the scroll extent. A reload
    // or full re-render lands on 0, which this catches.
    ensure!(
        (now - noted).abs() < 80.0,
        "the sidebar moved from {noted} to {now}"
    );
    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn expect_testid_active(world: &RdrsWorld, id: &str) -> Result<()> {
    eventually(&format!("`{id}` is active"), || async {
        let Some(item) = world.driver()?.test_id_opt(id).await? else {
            return Ok(false);
        };
        has_class(&item, "active").await
    })
    .await
}

async fn has_class(element: &WebElement, class: &str) -> Result<bool> {
    Ok(element
        .class_name()
        .await?
        .unwrap_or_default()
        .split_whitespace()
        .any(|name| name == class))
}

async fn first_containing(
    world: &RdrsWorld,
    selector: &str,
    text: &str,
) -> Result<Option<WebElement>> {
    for element in world.driver()?.css_all(selector).await? {
        if element
            .content_text()
            .await
            .unwrap_or_default()
            .contains(text)
        {
            return Ok(Some(element));
        }
    }
    Ok(None)
}

async fn category_link_opt(world: &RdrsWorld, name: &str) -> Result<Option<WebElement>> {
    first_containing(world, CATEGORY_LINK, name).await
}

async fn category_link(world: &RdrsWorld, name: &str) -> Result<WebElement> {
    wait_for(world, CATEGORY_LINK, name, "category").await
}

async fn feed_link_opt(world: &RdrsWorld, title: &str) -> Result<Option<WebElement>> {
    first_containing(world, FEED_LINK, title).await
}

async fn feed_link(world: &RdrsWorld, title: &str) -> Result<WebElement> {
    wait_for(world, FEED_LINK, title, "feed").await
}

async fn wait_for(world: &RdrsWorld, selector: &str, text: &str, what: &str) -> Result<WebElement> {
    eventually_some(&format!("a sidebar {what} named {text:?}"), || {
        first_containing(world, selector, text)
    })
    .await
}
