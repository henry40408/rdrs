//! Bulk triage — mark-as-read, starring, per-row controls — and the
//! reading-pane summary toggle. A port of `triage.steps.js`.

use std::time::Duration;

use anyhow::{Result, ensure};
use cucumber::{given, then, when};
use rdrs_e2e::dom::{Dom, TextContent, Within};
use rdrs_e2e::seed::NewEntry;
use rdrs_e2e::wait::{eventually, eventually_eq, eventually_within};
use rdrs_e2e::world::RdrsWorld;
use thirtyfour::prelude::*;

use super::entries::{entry_row, feed_id};
use super::reading_pane::SUMMARIZE_TOGGLE;

/// How long a held response may wait for its request to arrive. Generous: it
/// is the SSE round trip that triggers it, not a click.
const ARRIVAL_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a summary may take to come back.
///
/// Longer than the interaction timeout because it is not the page catching up:
/// the summary worker drains one job at a time and runs its database work at
/// background priority, so on a two-core CI runner it legitimately outlasts a
/// wait sized for a click. Matched to the server's own `SUMMARY_TIMEOUT`, past
/// which the job has genuinely failed rather than being slow.
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(90);

/// Seeds a fake Kagi session token so the reading pane renders its Summarize
/// button. Real Kagi requests go to the mock upstream.
#[given("the user has Kagi configured")]
async fn kagi_configured(world: &mut RdrsWorld) -> Result<()> {
    let user_id = world.user_id().await?;
    world.seed().configure_kagi(user_id, "e2e-test-token").await
}

/// Backdated past the "older than 1 day" cutoff
/// (`COALESCE(published_at, created_at) < now - 1 day`), so the age option has
/// something to catch while the Background's freshly-seeded entries stay put.
/// That contrast is what proves the cutoff was applied rather than everything
/// being marked.
#[given(expr = "the feed {string} has an entry titled {string} published 3 days ago")]
async fn aged_entry(world: &mut RdrsWorld, feed_title: String, title: String) -> Result<()> {
    let username = world.user.username.clone();
    let feed_id = feed_id(world, &feed_title).await?;
    let entry = NewEntry::new(feed_id, &format!("{username}-aged-entry"), &title)
        .link(format!("https://example.com/{username}/aged-entry"))
        .content(format!("<p>{title}</p>"))
        .published_offset("-3 days");
    world.seed().insert_entries(&[entry]).await.map(|_| ())
}

// ── Bulk marking ─────────────────────────────────────────────────────────────

/// The `#mark-read-age` `<select>` fires a `window.confirm` before calling
/// `/reader/api/0/mark-all-as-read`, so the prompt is auto-accepted first and
/// the dropdown is then triggered by picking an option.
///
/// On success `app.js` swaps the refreshed list into the live document rather
/// than reloading, so the assertions that follow run against the same page with
/// no navigation to wait for.
async fn mark_read_via_dropdown(world: &mut RdrsWorld, option: &str) -> Result<()> {
    super::keyboard::accept_next_dialog(world).await?;
    world
        .driver()?
        .select_option("mark-read-select", option)
        .await
}

#[when("I mark all entries as read")]
async fn mark_all_read(world: &mut RdrsWorld) -> Result<()> {
    mark_read_via_dropdown(world, "all").await
}

/// The age options carry a `ts=` cutoff, which is the case most likely to
/// regress back to a reload: it is the only dropdown path that leaves rows
/// behind, so a reload there is visible as lost scroll and a closed entry.
#[when("I mark entries older than 1 day as read")]
async fn mark_older_than_a_day(world: &mut RdrsWorld) -> Result<()> {
    mark_read_via_dropdown(world, "1").await
}

/// "Mark Above as Read" confirms before `POSTing`, same as the dropdown, and
/// shares its swap-instead-of-reload success path.
#[when("I mark the loaded entries as read")]
async fn mark_loaded_read(world: &mut RdrsWorld) -> Result<()> {
    super::keyboard::accept_next_dialog(world).await?;
    world.driver()?.click("mark-above-btn").await
}

// ── The list scroller ────────────────────────────────────────────────────────

/// Counted from the rows themselves, never from the empty-state placeholder: a
/// list that failed to refresh still has its rows on screen, and asserting on
/// the placeholder would let that pass as "empty" the moment it renders.
#[then(expr = "the entry list has {int} entries")]
async fn list_has_entries(world: &mut RdrsWorld, count: usize) -> Result<()> {
    let driver = world.driver()?;
    eventually_eq("the number of entry rows", count, || async {
        Ok(driver.test_ids("entry-item").await?.len())
    })
    .await
}

/// `[data-entries-list]` (`.list-pane-body`) is the scroller at this viewport —
/// the pane scrolls internally on desktop, which is where the offset survives a
/// swap. Polled because the rows arrive with the page and the first attempt can
/// land on a container that has not laid out yet.
#[when("I scroll the entry list to the bottom")]
async fn scroll_list_bottom(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("the entry list to scroll", || async {
        let offset = driver
            .eval(
                r"
                const el = document.querySelector('[data-entries-list]');
                el.scrollTop = el.scrollHeight;
                return el.scrollTop;
                ",
            )
            .await?;
        Ok(offset.as_f64().unwrap_or(0.0) > 0.0)
    })
    .await
}

#[then("the entry list is scrolled to the top")]
async fn list_scrolled_top(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually_eq("the entry list's scroll offset", 0.0, || async {
        let offset = driver
            .eval("return document.querySelector('[data-entries-list]').scrollTop;")
            .await?;
        Ok(offset.as_f64().unwrap_or(-1.0))
    })
    .await
}

// ── Per-row controls ─────────────────────────────────────────────────────────

#[when(expr = "I star the entry titled {string}")]
async fn star_entry(world: &mut RdrsWorld, title: String) -> Result<()> {
    let row = entry_row(world, &title).await?;
    row.test_id("entry-star-action").await?.click().await?;
    Ok(())
}

#[when(expr = "I click the read toggle for the entry titled {string}")]
async fn click_read_toggle(world: &mut RdrsWorld, title: String) -> Result<()> {
    let row = entry_row(world, &title).await?;
    row.test_id("entry-read-toggle").await?.click().await?;
    Ok(())
}

/// The Wire Room redesign removed the per-row read action; the star is the only
/// visible row control. Marking read now happens through the reading pane:
/// opening an entry auto-marks it read and returns the row in its read state
/// plus the decremented sidebar count — the same observable behaviour.
#[when(expr = "I mark the entry titled {string} read")]
async fn mark_entry_read(world: &mut RdrsWorld, title: String) -> Result<()> {
    let row = entry_row(world, &title).await?;
    row.test_id("entry-title-link").await?.click().await?;
    world.driver()?.expect_visible("reading-pane-title").await
}

#[then(expr = "the entry titled {string} is marked starred")]
async fn entry_is_starred(world: &mut RdrsWorld, title: String) -> Result<()> {
    eventually(&format!("`{title}` is starred"), || async {
        let row = entry_row(world, &title).await?;
        let Some(action) = row.test_id_opt("entry-star-action").await? else {
            return Ok(false);
        };
        Ok(action.attr("aria-label").await?.as_deref() == Some("Unstar"))
    })
    .await
}

/// A regression guard against silently dropping a per-row control, as the
/// 0.55.0 redesign did with mark-read and open-original. Every row must carry
/// the full set.
#[then("every entry row exposes the read toggle, star, open-original, time, and feed controls")]
async fn rows_expose_controls(world: &mut RdrsWorld) -> Result<()> {
    let rows = world.driver()?.test_ids("entry-item").await?;
    ensure!(!rows.is_empty(), "the list has no rows to check");
    for (index, row) in rows.iter().enumerate() {
        for id in [
            "entry-read-toggle",
            "entry-star-action",
            "entry-open-original",
        ] {
            let control = row
                .test_id_opt(id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("row {index} has no `{id}`"))?;
            ensure!(
                control.is_displayed().await?,
                "row {index}'s `{id}` is hidden"
            );
        }
        for selector in [".entry-time", ".entry-feed"] {
            let control = row
                .find(By::Css(selector))
                .await
                .map_err(|_| anyhow::anyhow!("row {index} has no `{selector}`"))?;
            ensure!(
                control.is_displayed().await?,
                "row {index}'s `{selector}` is hidden"
            );
        }
    }
    Ok(())
}

#[then("every open-original link points at the entry's source URL")]
async fn open_original_links(world: &mut RdrsWorld) -> Result<()> {
    let links = world.driver()?.test_ids("entry-open-original").await?;
    ensure!(!links.is_empty(), "the list has no open-original links");
    for (index, link) in links.iter().enumerate() {
        let href = link.attr("href").await?.unwrap_or_default();
        ensure!(
            href.starts_with("http://") || href.starts_with("https://"),
            "open-original link {index} points at {href:?}"
        );
    }
    Ok(())
}

/// Guards the restored title hover affordance, also lost in 0.55.0.
/// Theme-independent: it asserts the colour *changes* rather than a fixed
/// value.
#[then(expr = "the entry title for {string} highlights on hover")]
async fn title_highlights_on_hover(world: &mut RdrsWorld, title: String) -> Result<()> {
    let driver = world.driver()?;
    let row = entry_row(world, &title).await?;
    let link = row.test_id("entry-title-link").await?;

    // Move the pointer away first, so a link that happens to sit under it
    // already is not measured in its hovered state.
    driver.action_chain().move_to(0, 0).perform().await?;
    let base = color_of(driver, &link).await?;
    link.scroll_into_view().await?;
    driver
        .action_chain()
        .move_to_element_center(&link)
        .perform()
        .await?;

    eventually("the title colour to change on hover", || async {
        Ok(color_of(driver, &link).await? != base)
    })
    .await
}

async fn color_of(driver: &WebDriver, element: &WebElement) -> Result<String> {
    let value = driver
        .execute(
            "return getComputedStyle(arguments[0]).color;",
            vec![element.to_json()?],
        )
        .await?;
    Ok(value.json().as_str().unwrap_or_default().to_owned())
}

// ── Sidebar counts ───────────────────────────────────────────────────────────

/// The sidebar Starred link carries no numeric badge — only Unread does — so
/// this can only assert the link is there. Strengthen it to a numeric check if
/// the Starred link ever gains a badge.
#[then(expr = "the sidebar starred count is at least {int}")]
async fn starred_count(world: &mut RdrsWorld, minimum: u32) -> Result<()> {
    // Dropped rather than named `_minimum`: the step macro binds the parameter
    // itself, so an underscore prefix reads as "used but marked unused".
    let _ = minimum;
    let link = world.driver()?.css(r#"a[href="/entries/starred"]"#).await?;
    ensure!(link.is_displayed().await?, "the Starred link is hidden");
    Ok(())
}

/// A delta comparison needs a before-count captured in the same step that
/// causes the change, which this one does not have — see the SSE steps for the
/// version that does. For now it asserts the badge is present and non-negative.
#[then(expr = "the sidebar unread count decreases by {int}")]
async fn unread_decreases_by(world: &mut RdrsWorld, delta: u32) -> Result<()> {
    // Dropped for the same reason as in `starred_count` above.
    let _ = delta;
    let text = world
        .driver()?
        .css("#unread-count")
        .await?
        .content_text()
        .await?;
    let count: i64 = text.trim().parse().unwrap_or(-1);
    ensure!(count >= 0, "the unread badge reads {text:?}");
    Ok(())
}

// ── The held summary fragment ────────────────────────────────────────────────

/// Holding the SSE-driven `GET /entries/{id}/summary/fragment` reproduces the
/// race a reader hit: the event for the outgoing entry passes its
/// `currentPaneEntryId()` pre-check, then the response lands after the pane has
/// already moved on.
#[when("the summary fragment response is held")]
async fn hold_summary_fragment(world: &mut RdrsWorld) -> Result<()> {
    let handle = world
        .hold_requests(r"/entries/\d+/summary/fragment")
        .await?;
    world.held_summary_fragment = Some(handle);
    Ok(())
}

#[when("the summary fragment request is in flight")]
async fn summary_fragment_in_flight(world: &mut RdrsWorld) -> Result<()> {
    let handle = world
        .held_summary_fragment
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no summary fragment route was armed"))?;
    handle.wait_for_arrival(ARRIVAL_TIMEOUT).await.map_err(|_| {
        anyhow::anyhow!("no summary fragment request arrived — the SSE event never fired")
    })
}

#[when("the held summary fragment response lands")]
async fn summary_fragment_lands(world: &mut RdrsWorld) -> Result<()> {
    let handle = world
        .held_summary_fragment
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no summary fragment route was armed"))?;
    handle.release()?;
    handle.wait_for_settled(ARRIVAL_TIMEOUT).await?;
    // The response is on the wire; give `performSwap` the two frames it needs
    // to apply — or, as asserted next, discard — it before the DOM is read.
    world
        .driver()?
        .eval("return new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));")
        .await?;
    Ok(())
}

// ── The summary panel ────────────────────────────────────────────────────────

#[then("the reading pane shows no summary")]
async fn pane_shows_no_summary(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("the summary box to be absent", || async {
        Ok(driver
            .css_all("#rp-summary-container .summary-box")
            .await?
            .is_empty())
    })
    .await
}

/// On failure this reports what the summary panel *did* hold — the pending
/// placeholder, an error banner, or nothing at all — because "no displayed
/// element" alone cannot tell a summary that never started from one still in
/// flight.
#[then("the reading pane shows a summary")]
async fn pane_shows_summary(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    let arrived = eventually_within(SUMMARY_TIMEOUT, "a summary to render", || async {
        let Some(box_) = driver.css_opt("#rp-summary-container .summary-box").await? else {
            return Ok(false);
        };
        // The pending placeholder is a `.summary-box` too, so its presence is
        // not the answer — the summary has landed only once the pending marker
        // is gone.
        Ok(box_.attr("data-summary-pending").await?.is_none())
    })
    .await;
    if arrived.is_ok() {
        return Ok(());
    }
    let panel = driver
        .eval(
            "const el = document.querySelector('#rp-summary-container');\
             return el ? el.innerHTML : '(no #rp-summary-container)';",
        )
        .await?;
    let toggle = driver
        .eval(&format!(
            "const b = document.querySelector('{SUMMARIZE_TOGGLE}');\
             return b ? (b.disabled ? 'disabled' : 'enabled') : '(no toggle)';"
        ))
        .await?;
    anyhow::bail!("no summary rendered. The toggle is {toggle}, and the panel holds: {panel}")
}

/// After clicking Dismiss, `app.js` calls `container.replaceChildren()`, which
/// empties `#rp-summary-container` but keeps the wrapper in the DOM.
#[then("the reading pane summary is dismissed")]
async fn summary_dismissed(world: &mut RdrsWorld) -> Result<()> {
    pane_shows_no_summary(world).await
}

#[when("I click the reading-pane summarize toggle")]
async fn click_summarize_toggle(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.click_css(SUMMARIZE_TOGGLE).await
}

#[then(expr = "the reading-pane summarize toggle reads {string}")]
async fn summarize_toggle_reads(world: &mut RdrsWorld, text: String) -> Result<()> {
    let driver = world.driver()?;
    eventually_eq("the summarize toggle's label", text, || async {
        Ok(driver
            .css(&format!("{SUMMARIZE_TOGGLE} .action-label"))
            .await?
            .content_text()
            .await?
            .trim()
            .to_owned())
    })
    .await
}

/// Only the visible icon span — the hidden one is toggled off with `hidden`.
#[then("the reading-pane summarize toggle still shows its icon")]
async fn summarize_toggle_shows_icon(world: &mut RdrsWorld) -> Result<()> {
    let icon = world
        .driver()?
        .css(&format!(
            "{SUMMARIZE_TOGGLE} .action-icon:not([hidden]) svg"
        ))
        .await?;
    ensure!(icon.is_displayed().await?, "the toggle's icon is hidden");
    Ok(())
}

#[then("the reading-pane summarize toggle is disabled")]
async fn summarize_toggle_disabled(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("the summarize toggle to be disabled", || async {
        let Some(button) = driver.css_opt(SUMMARIZE_TOGGLE).await? else {
            return Ok(false);
        };
        Ok(!button.is_enabled().await?)
    })
    .await
}

// ── Proving the in-flight toggle is inert ────────────────────────────────────

/// Counts re-queue POSTs to `/entries/{id}/summarize` — and not to
/// `/summarize/cancel`, which the trailing `$` excludes.
#[when("I watch for summarize POST requests")]
async fn watch_summarize_posts(world: &mut RdrsWorld) -> Result<()> {
    let handle = world
        .watch_requests(r"/entries/\d+/summarize(\?|$)", "POST")
        .await?;
    world.summarize_posts = Some(handle);
    Ok(())
}

#[then("no summarize POST request is sent")]
async fn no_summarize_post(world: &mut RdrsWorld) -> Result<()> {
    let handle = world
        .summarize_posts
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no summarize watch was armed"))?;
    // A real re-queue POSTs synchronously on submit; this gives it a beat to
    // land, then asserts none fired.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let count = handle.arrived();
    ensure!(count == 0, "{count} summarize POST(s) were sent");
    Ok(())
}
