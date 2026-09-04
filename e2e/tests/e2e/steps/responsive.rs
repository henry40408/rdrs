//! Viewports, the mobile drawer, the statistics chart and the flash banner —
//! a port of `responsive.steps.js`.

use anyhow::{Result, ensure};
use cucumber::{given, then, when};
use rdrs_e2e::browser::Viewport;
use rdrs_e2e::dom::{Dom, TextContent};
use rdrs_e2e::wait::eventually;
use rdrs_e2e::world::RdrsWorld;
use thirtyfour::prelude::*;

/// The named viewports the scenarios switch between.
fn viewport(kind: &str) -> Option<Viewport> {
    Some(match kind {
        "mobile" => Viewport::new(375, 667),
        "tablet" => Viewport::new(768, 1024),
        "desktop" => Viewport::new(1280, 800),
        "wide" => Viewport::new(1400, 900),
        _ => return None,
    })
}

#[given(expr = "I am viewing on a {word} screen")]
async fn viewing_on_screen(world: &mut RdrsWorld, kind: String) -> Result<()> {
    let viewport = viewport(&kind).ok_or_else(|| anyhow::anyhow!("unknown viewport: {kind}"))?;
    world.resize(viewport).await
}

#[given(expr = "I have a feed with {int} test entries")]
async fn feed_with_entries(world: &mut RdrsWorld, count: u32) -> Result<()> {
    seed_feed(world, "Mobile Feed", count).await.map(|_| ())
}

/// One read entry per day across the default 7-day window, so the chart renders
/// a full row of bars — including the rightmost, whose tooltip is what used to
/// overflow the viewport.
#[given("I have read entries across several days")]
async fn read_entries_across_days(world: &mut RdrsWorld) -> Result<()> {
    seed_read_history(world, 8).await
}

/// One read entry per day across ~30 days, so a 90-day range has plenty of
/// activity to bucket into bars.
#[given("I have read entries spanning several weeks")]
async fn read_entries_across_weeks(world: &mut RdrsWorld) -> Result<()> {
    seed_read_history(world, 30).await
}

async fn seed_read_history(world: &mut RdrsWorld, days: u32) -> Result<()> {
    let ids = seed_feed(world, "Stats Feed", days).await?;
    let seed = world.seed().clone();
    for (day, id) in ids.iter().enumerate() {
        seed.mark_read(*id, &format!("-{day} days")).await?;
    }
    Ok(())
}

async fn seed_feed(world: &mut RdrsWorld, title: &str, count: u32) -> Result<Vec<i64>> {
    let username = world.user.username.clone();
    let user_id = world.user_id().await?;
    let seed = world.seed().clone();
    let category_id = seed
        .create_category(user_id, &format!("Cat-{username}"))
        .await?;
    let feed_id = seed
        .create_feed(
            category_id,
            &format!("https://example.com/{username}.xml"),
            Some(title),
        )
        .await?;
    let ids = seed.seed_test_entries(feed_id, count).await?;
    world.seeded_entries.clone_from(&ids);
    Ok(ids)
}

// ── Navigation ───────────────────────────────────────────────────────────────

#[when("I open the inbox")]
async fn open_inbox(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/").await
}

#[when("I open the categories page")]
async fn open_categories(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/categories").await
}

#[when("I open the all-entries page")]
async fn open_all_entries(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/entries").await
}

#[when("I open the feeds page")]
async fn open_feeds(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/feeds").await
}

#[when("I open the import page")]
async fn open_import(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/feeds/import").await
}

#[when(expr = "I open the statistics page for the {string} period")]
async fn open_statistics_period(world: &mut RdrsWorld, period: String) -> Result<()> {
    world.goto(&format!("/statistics?period={period}")).await
}

#[when(expr = "I open the edit page for feed {string}")]
async fn open_feed_edit(world: &mut RdrsWorld, feed_title: String) -> Result<()> {
    let table = world.driver()?.test_id("feeds-table").await?;
    for row in table.find_all(By::Tag("tr")).await? {
        if !row
            .content_text()
            .await
            .unwrap_or_default()
            .contains(&feed_title)
        {
            continue;
        }
        let link = row
            .query(By::XPath(".//a[normalize-space(.)='edit']"))
            .nowait()
            .first_opt()
            .await?
            .ok_or_else(|| anyhow::anyhow!("the row for `{feed_title}` has no edit link"))?;
        link.click().await?;
        return Ok(());
    }
    anyhow::bail!("the feeds table has no row for `{feed_title}`")
}

#[when(expr = "I expand the {string} disclosure")]
async fn expand_disclosure(world: &mut RdrsWorld, label: String) -> Result<()> {
    let driver = world.driver()?;
    for summary in driver.css_all("summary").await? {
        if summary
            .content_text()
            .await
            .unwrap_or_default()
            .contains(&label)
        {
            summary.click().await?;
            return Ok(());
        }
    }
    anyhow::bail!("no disclosure is labelled `{label}`")
}

// ── The mobile drawer ────────────────────────────────────────────────────────

#[when("I tap the hamburger")]
async fn tap_hamburger(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.click_css(".sidebar-toggle").await
}

#[when("I tap the sidebar close button")]
async fn tap_sidebar_close(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.click_css(".sidebar-close").await
}

/// Clicks the dimmed area on the right half of the viewport, well clear of the
/// left-anchored drawer — exercising the document-level tap-outside close.
#[when("I tap outside the sidebar")]
async fn tap_outside_sidebar(world: &mut RdrsWorld) -> Result<()> {
    let viewport = world.browser()?.viewport();
    let x = i64::from(viewport.width) - 10;
    let y = i64::from(viewport.height / 2);
    world
        .driver()?
        .action_chain()
        .move_to(x, y)
        .click()
        .perform()
        .await?;
    Ok(())
}

#[then("the sidebar is visible")]
async fn sidebar_open(world: &mut RdrsWorld) -> Result<()> {
    expect_sidebar_open(world, true).await
}

#[then("the sidebar is not visible")]
async fn sidebar_closed(world: &mut RdrsWorld) -> Result<()> {
    expect_sidebar_open(world, false).await
}

async fn expect_sidebar_open(world: &RdrsWorld, open: bool) -> Result<()> {
    let driver = world.driver()?;
    eventually(
        &format!("the sidebar to be {}", if open { "open" } else { "closed" }),
        || async {
            let Some(sidebar) = driver.css_opt("#sidebar").await? else {
                return Ok(!open);
            };
            let classes = sidebar.class_name().await?.unwrap_or_default();
            Ok(classes.split_whitespace().any(|name| name == "open") == open)
        },
    )
    .await
}

#[then("the hamburger button is visible")]
async fn hamburger_visible(world: &mut RdrsWorld) -> Result<()> {
    let toggle = world.driver()?.css(".sidebar-toggle").await?;
    ensure!(toggle.is_displayed().await?, "the hamburger is hidden");
    Ok(())
}

#[then("the sidebar is always-visible")]
async fn sidebar_always_visible(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    let sidebar = driver.css("#sidebar").await?;
    ensure!(sidebar.is_displayed().await?, "the sidebar is hidden");
    let toggle = driver.css_opt(".sidebar-toggle").await?;
    if let Some(toggle) = toggle {
        ensure!(
            !toggle.is_displayed().await?,
            "the hamburger is still shown at this width"
        );
    }
    Ok(())
}

// ── Layout ───────────────────────────────────────────────────────────────────

#[then(expr = "the entry list pane is at least {int}px wide")]
async fn list_pane_at_least(world: &mut RdrsWorld, min_width: f64) -> Result<()> {
    let (_, _, width, _) = world.driver()?.bounding_box(".list-pane").await?;
    ensure!(width >= min_width, "the list pane is {width}px wide");
    Ok(())
}

#[then("the entry list pane is narrower than the viewport")]
async fn list_pane_narrower(world: &mut RdrsWorld) -> Result<()> {
    let viewport = world.browser()?.viewport();
    let (_, _, width, _) = world.driver()?.bounding_box(".list-pane").await?;
    let limit = f64::from(viewport.width) * 0.9;
    ensure!(
        width < limit,
        "the list pane is {width}px wide in a {}px viewport",
        viewport.width
    );
    Ok(())
}

#[then("the categories table is shown as cards")]
async fn table_as_cards(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    let display = driver
        .computed_style("table.mobile-cards thead", "display")
        .await?;
    ensure!(
        display == "none",
        "the table head is `{display}`, not hidden"
    );
    let cell = driver.css("table.mobile-cards td[data-label]").await?;
    ensure!(cell.is_displayed().await?, "the card cells are hidden");
    Ok(())
}

#[then("the categories table is shown as a table")]
async fn table_as_table(world: &mut RdrsWorld) -> Result<()> {
    let display = world
        .driver()?
        .computed_style("table.mobile-cards thead", "display")
        .await?;
    ensure!(display != "none", "the table head is still hidden");
    Ok(())
}

#[then("the page has no horizontal scroll")]
async fn no_horizontal_scroll(world: &mut RdrsWorld) -> Result<()> {
    let overflow = world
        .driver()?
        .eval(
            "return document.documentElement.scrollWidth \
             - document.documentElement.clientWidth;",
        )
        .await?;
    let overflow = overflow.as_f64().unwrap_or_default();
    ensure!(overflow <= 0.0, "the page overflows by {overflow}px");
    Ok(())
}

/// The star and open-original cluster (`.rail-actions`) and the feed meta line
/// (`.entry-item-meta`) share grid row 2; their vertical centres must coincide.
/// The old absolute-overlay positioning used a hand-tuned `bottom` offset that
/// drifted on mobile (the meta grew via the feed-link tap padding), leaving the
/// actions several px low.
#[then("the entry-row actions are vertically centered on the meta line")]
async fn row_actions_centered(world: &mut RdrsWorld) -> Result<()> {
    let row = world.driver()?.test_id("entry-item").await?;
    let delta = world
        .driver()?
        .execute(
            r"
            const n = arguments[0];
            const c = (sel) => {
              const r = n.querySelector(sel).getBoundingClientRect();
              return r.y + r.height / 2;
            };
            return Math.abs(c('.rail-actions') - c('.entry-item-meta'));
            ",
            vec![row.to_json()?],
        )
        .await?;
    let delta = delta.json().as_f64().unwrap_or(f64::MAX);
    ensure!(delta <= 1.5, "the actions sit {delta}px off the meta line");
    Ok(())
}

/// The filter bar holds the status filter, the mark-as-read select and the
/// search box, and uses `flex-wrap`, so inside the fixed-width list pane it may
/// legitimately wrap onto a second row — that is by design and not what this
/// guards. The real invariant is that nothing overflows the pane horizontally.
#[then("the entry-list filter bar does not overflow the list pane")]
async fn filter_bar_fits(world: &mut RdrsWorld) -> Result<()> {
    let pane = world.driver()?.css(".list-pane").await?;
    ensure!(pane.is_displayed().await?, "the list pane is hidden");
    let overflow = world
        .driver()?
        .execute(
            r"
            const p = arguments[0];
            const paneRight = p.getBoundingClientRect().right;
            const groups = [...p.querySelectorAll('.filter-bar > .form-group')];
            const pastRight = Math.max(
              0,
              ...groups.map((g) => g.getBoundingClientRect().right - paneRight),
            );
            return {
              count: groups.length,
              horizontalScroll: p.scrollWidth - p.clientWidth,
              pastRight,
            };
            ",
            vec![pane.to_json()?],
        )
        .await?;
    let overflow = overflow.json();
    let count = overflow["count"].as_u64().unwrap_or(0);
    ensure!(count >= 2, "the filter bar renders only {count} control(s)");
    // Sub-pixel tolerance for rounding.
    let past_right = overflow["pastRight"].as_f64().unwrap_or_default();
    let scroll = overflow["horizontalScroll"].as_f64().unwrap_or_default();
    ensure!(
        past_right <= 1.0,
        "a control reaches {past_right}px past the pane"
    );
    ensure!(scroll <= 1.0, "the pane scrolls {scroll}px horizontally");
    Ok(())
}

/// Sub-pixel slack for the size assertions.
///
/// `getBoundingClientRect` reports fractional layout, so a control laid out to
/// exactly 44px can measure 43.99999237 — Playwright's `boundingBox()` never
/// showed this because it reads the CDP box model, whose quads are already
/// quantised. A tap target off by a ten-thousandth of a pixel is the same tap
/// target; anything that actually regresses is out by whole pixels.
const SUBPIXEL: f64 = 0.5;

#[then(expr = "the {string} control is at least {int}px tall")]
async fn control_at_least_tall(world: &mut RdrsWorld, selector: String, min: f64) -> Result<()> {
    let (_, _, _, height) = world.driver()?.bounding_box(&selector).await?;
    ensure!(
        height >= min - SUBPIXEL,
        "`{selector}` is {height}px tall, short of {min}"
    );
    Ok(())
}

#[then(expr = "the {string} control is at least {int}px wide")]
async fn control_at_least_wide(world: &mut RdrsWorld, selector: String, min: f64) -> Result<()> {
    let (_, _, width, _) = world.driver()?.bounding_box(&selector).await?;
    ensure!(
        width >= min - SUBPIXEL,
        "`{selector}` is {width}px wide, short of {min}"
    );
    Ok(())
}

/// Every control in the reading pane's action bar, as `(label, x, width)`.
///
/// On mobile the bar is a fixed strip whose buttons are laid out by flex, so a
/// label that changes width — "Summarize" → "Summarizing…" → "Dismiss", "Star"
/// → "Starred" — is exactly the input that must *not* move anything.
async fn action_bar_boxes(world: &RdrsWorld) -> Result<Vec<(String, f64, f64)>> {
    let boxes = world
        .driver()?
        .eval(
            "return [...document.querySelectorAll('.reading-pane-actions .rp-action')]\
               .map((b) => {\
                 const r = b.getBoundingClientRect();\
                 const l = b.querySelector('.action-label');\
                 return [l ? l.textContent.trim() : '', r.x, r.width];\
               });",
        )
        .await?;
    let rows = boxes
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("the action-bar probe did not return an array"))?
        .iter()
        .map(|row| {
            (
                row[0].as_str().unwrap_or_default().to_owned(),
                row[1].as_f64().unwrap_or_default(),
                row[2].as_f64().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    ensure!(!rows.is_empty(), "the reading pane has no action bar");
    Ok(rows)
}

fn describe(boxes: &[(String, f64, f64)]) -> String {
    boxes
        .iter()
        .map(|(label, x, width)| format!("{label} @{x:.1}+{width:.1}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[when("I record the reading-pane action bar layout")]
async fn record_action_bar(world: &mut RdrsWorld) -> Result<()> {
    world.action_bar = Some(action_bar_boxes(world).await?);
    Ok(())
}

/// The bar's slots must stay the same size in the same places whatever the
/// labels now read — otherwise every button slides sideways under the thumb
/// that just tapped one.
#[then("the reading-pane action bar layout is unchanged")]
async fn action_bar_unchanged(world: &mut RdrsWorld) -> Result<()> {
    let before = world
        .action_bar
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no action-bar layout was recorded"))?;
    let after = action_bar_boxes(world).await?;
    ensure!(
        before.len() == after.len(),
        "the bar had {} controls and now has {}:\n  before: {}\n  after:  {}",
        before.len(),
        after.len(),
        describe(&before),
        describe(&after)
    );
    for (old, new) in before.iter().zip(&after) {
        let moved = (old.1 - new.1).abs();
        let resized = (old.2 - new.2).abs();
        ensure!(
            moved <= SUBPIXEL && resized <= SUBPIXEL,
            "`{}` moved {moved:.1}px and resized {resized:.1}px:\n  before: {}\n  after:  {}",
            old.0,
            describe(&before),
            describe(&after)
        );
    }
    Ok(())
}

#[then(expr = "the {string} element is visible")]
async fn element_visible(world: &mut RdrsWorld, selector: String) -> Result<()> {
    let element = world.driver()?.css(&selector).await?;
    ensure!(element.is_displayed().await?, "`{selector}` is hidden");
    Ok(())
}

// ── The reading pane overlay ─────────────────────────────────────────────────

/// At ≤1024px the reading pane is `display: none` by default and only surfaces
/// when `.reading-pane-active` is present. Both are asserted: the class alone
/// would pass even if a future CSS regression unset `display: block`.
#[then("the reading pane is visible on mobile")]
async fn pane_visible_on_mobile(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("the reading pane overlay to open", || async {
        let Some(pane) = driver.css_opt("#reading-pane").await? else {
            return Ok(false);
        };
        let active = pane
            .class_name()
            .await?
            .unwrap_or_default()
            .split_whitespace()
            .any(|name| name == "reading-pane-active");
        Ok(active && pane.is_displayed().await?)
    })
    .await
}

#[when("I tap the reading-pane back button")]
async fn tap_pane_back(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.click("reading-pane-back").await
}

/// `closeReadingPane()` strips `.reading-pane-active` and restores the empty
/// placeholder; at ≤1024px the pane without the active class is
/// `display: none`, so it must be both class-free and actually hidden.
#[then("the reading pane overlay is dismissed")]
async fn pane_overlay_dismissed(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("the reading pane overlay to close", || async {
        let Some(pane) = driver.css_opt("#reading-pane").await? else {
            return Ok(true);
        };
        let active = pane
            .class_name()
            .await?
            .unwrap_or_default()
            .split_whitespace()
            .any(|name| name == "reading-pane-active");
        Ok(!active && !pane.is_displayed().await?)
    })
    .await
}

// ── The statistics chart ─────────────────────────────────────────────────────

#[when("I hover the last daily-read bar")]
async fn hover_last_bar(world: &mut RdrsWorld) -> Result<()> {
    let bars = world.driver()?.css_all(".stats-bar-col").await?;
    let bar = bars
        .last()
        .ok_or_else(|| anyhow::anyhow!("the chart renders no bars"))?;
    hover(world, bar).await
}

#[when(expr = "I hover daily-read bar number {int}")]
async fn hover_nth_bar(world: &mut RdrsWorld, n: usize) -> Result<()> {
    let bars = world.driver()?.css_all(".stats-bar-col").await?;
    let bar = bars
        .get(n - 1)
        .ok_or_else(|| anyhow::anyhow!("the chart has fewer than {n} bars"))?;
    hover(world, bar).await
}

async fn hover(world: &RdrsWorld, element: &WebElement) -> Result<()> {
    element.scroll_into_view().await?;
    world
        .driver()?
        .action_chain()
        .move_to_element_center(element)
        .perform()
        .await?;
    Ok(())
}

#[then("the visible daily-read tooltip is within the viewport")]
async fn tooltip_within_viewport(world: &mut RdrsWorld) -> Result<()> {
    let rect = world
        .driver()?
        .eval(
            r"
            const tip = [...document.querySelectorAll('.stats-bar-tip')].find(
              (t) => getComputedStyle(t).visibility === 'visible',
            );
            if (!tip) return null;
            const r = tip.getBoundingClientRect();
            return { left: r.left, right: r.right, vw: document.documentElement.clientWidth };
            ",
        )
        .await?;
    ensure!(!rect.is_null(), "no tooltip is visible");
    let left = rect["left"].as_f64().unwrap_or_default();
    let right = rect["right"].as_f64().unwrap_or_default();
    let width = rect["vw"].as_f64().unwrap_or_default();
    ensure!(
        left >= -0.5,
        "the tooltip starts {left}px off the left edge"
    );
    ensure!(
        right <= width + 0.5,
        "the tooltip reaches {right}px in a {width}px viewport"
    );
    Ok(())
}

#[then("the daily-read chart is visible")]
async fn chart_visible(world: &mut RdrsWorld) -> Result<()> {
    let chart = world.driver()?.css(".stats-chart").await?;
    ensure!(chart.is_displayed().await?, "the chart is hidden");
    Ok(())
}

#[then(expr = "the daily-read chart has at most {int} bars")]
async fn chart_at_most_bars(world: &mut RdrsWorld, max: usize) -> Result<()> {
    let count = world.driver()?.css_all(".stats-bar-col").await?.len();
    ensure!(count > 0, "the chart renders no bars");
    ensure!(count <= max, "the chart renders {count} bars");
    Ok(())
}

#[then("some daily-read axis labels are hidden")]
async fn some_labels_hidden(world: &mut RdrsWorld) -> Result<()> {
    let labels = world.driver()?.css_all(".stats-bar-label").await?;
    ensure!(!labels.is_empty(), "the chart renders no axis labels");
    // `:visible` has no WebDriver equivalent, so each label is asked directly —
    // a driver-side computation, which is also why it survives scripting being
    // switched off.
    let mut visible = 0;
    for label in &labels {
        if label.is_displayed().await? {
            visible += 1;
        }
    }
    ensure!(
        visible < labels.len(),
        "all {} labels are shown, so none were thinned",
        labels.len()
    );
    Ok(())
}

#[then(expr = "the daily-read bars are each at least {int}px wide")]
async fn bars_at_least_wide(world: &mut RdrsWorld, min: f64) -> Result<()> {
    let bars = world.driver()?.css_all(".stats-bar-col").await?;
    ensure!(!bars.is_empty(), "the chart renders no bars");
    for (index, bar) in bars.iter().enumerate() {
        let width = bar.rect().await?.width;
        ensure!(width >= min, "bar {index} is {width}px wide");
    }
    Ok(())
}

// ── The flash banner ─────────────────────────────────────────────────────────

/// Drives the page-level `<rdrs-flash>` API directly — the same entry point the
/// app's own JS uses.
#[when("a flash banner is shown")]
async fn flash_shown(world: &mut RdrsWorld) -> Result<()> {
    world
        .driver()?
        .eval(
            "window.flash.show('success', 'Marked older than 1 week entries as read.');\
             return true;",
        )
        .await?;
    world.driver()?.css(".banner").await.map(|_| ())
}

#[then("the flash banner sits below the hamburger")]
async fn banner_below_hamburger(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    let (toggle_x, toggle_y, _, toggle_height) = driver.bounding_box(".sidebar-toggle").await?;
    let (banner_x, banner_y, _, _) = driver.bounding_box(".banner").await?;
    ensure!(
        banner_y >= toggle_y + toggle_height,
        "the banner overlaps the hamburger ({banner_y} vs {})",
        toggle_y + toggle_height
    );
    // …and full-width: the banner's left edge reaches past the floating
    // button's left edge instead of being indented to clear it.
    ensure!(
        banner_x < toggle_x,
        "the banner is indented to {banner_x}, clear of the button at {toggle_x}"
    );
    Ok(())
}

/// Reproduces iPad-landscape: a **wide** (>1024px, so the persistent split
/// layout rather than the mobile drawer) **touch** viewport. Touch triggers
/// `@media (hover: none)`, which bumps `.banner-dismiss` to 44px tall; the base
/// `.banner { align-items: start }` then pinned the message to the top of the
/// inflated grid row while the `align-self: center` timestamp sat lower.
///
/// Playwright could not change `hasTouch` on a live context and had to spin a
/// second one with the session's cookies copied across. CDP emulates it on the
/// session in place, so this stays in the same browser and the same sign-in.
#[then("the flash banner is vertically centered on a wide touch tablet")]
async fn banner_centered_on_touch_tablet(world: &mut RdrsWorld) -> Result<()> {
    world.resize(Viewport::new(1180, 820)).await?;
    world.browser()?.set_touch(true).await?;
    world.goto("/").await?;

    let driver = world.driver()?;
    driver
        .eval("window.flash.show('success', 'Marked as unread.'); return true;")
        .await?;
    let banner = driver.css(".banner").await?;
    let measurements = driver
        .execute(
            r"
            const n = arguments[0];
            const centerY = (sel) => {
              const r = n.querySelector(sel).getBoundingClientRect();
              return r.y + r.height / 2;
            };
            const d = n.querySelector('.banner-dismiss').getBoundingClientRect();
            return {
              msg: centerY('.banner-message'),
              time: centerY('.banner-time'),
              dismissW: d.width,
              dismissH: d.height,
            };
            ",
            vec![banner.to_json()?],
        )
        .await?;
    let m = measurements.json();
    let message = m["msg"].as_f64().unwrap_or_default();
    let time = m["time"].as_f64().unwrap_or_default();
    let dismiss_width = m["dismissW"].as_f64().unwrap_or_default();
    let dismiss_height = m["dismissH"].as_f64().unwrap_or_default();

    // Message and timestamp share the row's vertical centre.
    ensure!(
        (message - time).abs() <= 1.5,
        "the message and timestamp centres are {message} and {time}"
    );
    // Dismiss keeps a full 44px tap target on both axes at this width.
    ensure!(
        dismiss_width >= 44.0 && dismiss_height >= 44.0,
        "the dismiss target is {dismiss_width}×{dismiss_height}"
    );
    Ok(())
}

/// Every checkbox row in a form: the label's box against the box of whatever
/// follows it.
///
/// Checked over all of them rather than by test id, so a checkbox added later
/// is covered without anyone remembering to extend this. The 44px tap rule
/// makes these labels `inline-flex`, and an inline label lets the hint after it
/// continue along the label's own last line — which is the layout this catches.
#[then("every checkbox hint starts on its own line")]
async fn checkbox_hints_start_on_their_own_line(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    let measured = driver
        .eval(
            r#"
            return Array.from(document.querySelectorAll('input[type="checkbox"]'))
              .map((input) => {
                const label = input.closest('label');
                const hint = label && label.nextElementSibling;
                if (!label || !hint) return null;
                const box = label.getBoundingClientRect();
                return {
                  id: input.id,
                  labelBottom: box.bottom,
                  labelHeight: box.height,
                  hintTop: hint.getBoundingClientRect().top,
                };
              })
              .filter(Boolean);
            "#,
        )
        .await?;
    let rows = measured.as_array().cloned().unwrap_or_default();
    ensure!(
        !rows.is_empty(),
        "no checkbox is followed by a hint — this assertion would pass vacuously"
    );
    for row in rows {
        let id = row["id"].as_str().unwrap_or("(unnamed)").to_owned();
        let label_bottom = row["labelBottom"].as_f64().unwrap_or_default();
        let hint_top = row["hintTop"].as_f64().unwrap_or_default();
        // Half a pixel of slack for subpixel rounding. A hint sharing the
        // label's line sits a whole line-height above its bottom edge, not a
        // fraction of a pixel.
        ensure!(
            hint_top >= label_bottom - 0.5,
            "`{id}`'s hint starts at {hint_top}, above the label's bottom edge at {label_bottom} — it is running alongside the label instead of under it"
        );
        // The other half of the trade-off: the reason those labels are flex at
        // all is the 44px tap row, so pushing the hint down must not cost it.
        let label_height = row["labelHeight"].as_f64().unwrap_or_default();
        ensure!(
            label_height >= 44.0,
            "`{id}`'s tap row is only {label_height}px tall"
        );
    }
    Ok(())
}
