//! Opening entries, the reading pane, its neighbour navigation, and the
//! summary panel — including the scenarios that hold a response back to prove
//! a stale one never wins.
//!
//! Split out of `entries.steps.js` — see [`super::entries`].

use std::time::Duration;

use anyhow::{Result, ensure};
use cucumber::{then, when};
use rdrs_e2e::dom::{Dom, TextContent, Within, click_when_ready};
use rdrs_e2e::wait::{eventually, eventually_eq, settles};
use rdrs_e2e::world::RdrsWorld;

use super::entries::{entry_id, entry_row};

/// The action-bar Summarize/Dismiss toggle, located by its stable
/// `data-summary-toggle` marker rather than its accessible name — the name
/// flips between "Summarize" and "Dismiss summary" with summary state, and
/// "Dismiss summary" would otherwise collide with the summary box's own
/// Dismiss control.
pub const SUMMARIZE_TOGGLE: &str = ".reading-pane-actions [data-summary-toggle] button";

/// How long a held response is kept before being let through.
///
/// Long enough for the second click to land while the first is still in
/// flight, which is the whole point of the race scenarios.
const HOLD: Duration = Duration::from_millis(600);

/// A settled response gets this long to apply before the assertions run —
/// pre-fix, the bug shows up as the pane flipping back *after* this point.
const SETTLE: Duration = Duration::from_millis(100);

// ── Opening an entry ─────────────────────────────────────────────────────────

/// Clicks the title link rather than the row: `installRowClickToOpen` bails on
/// any `<a>` target, so the title link is the canonical open-entry action.
///
/// Waits for the reading-pane swap to complete — the empty placeholder loses
/// its `.reading-pane-empty` class once the fragment replaces `#reading-pane`.
#[when(expr = "I click the entry titled {string}")]
async fn click_entry(world: &mut RdrsWorld, title: String) -> Result<()> {
    open_entry(world, &title).await?;
    let driver = world.driver()?;
    eventually("the reading pane to fill", || async {
        Ok(driver
            .css_opt("#reading-pane:not(.reading-pane-empty)")
            .await?
            .is_some())
    })
    .await?;

    // The fragment landing is not the whole story: the pane's actions come
    // alive only once `/api/entries/{id}/neighbors` resolves, and a keystroke
    // aimed at one before then is a no-op — which is exactly what the "the
    // summarize toggle is inert" scenario asserts on purpose. Waiting for the
    // toggle's state to *stop changing* serves both readings: an entry with a
    // summary already in flight settles disabled, an ordinary one settles
    // enabled, and neither is assumed.
    //
    // Playwright never needed this: its per-step round trips took long enough
    // that the resolve had always landed by the next line.
    settles("the reading pane's actions", 3, || async {
        match driver.css_opt(SUMMARIZE_TOGGLE).await? {
            // No Kagi configured, so there is no toggle to settle.
            None => Ok(None),
            Some(button) => Ok(Some(button.is_enabled().await?)),
        }
    })
    .await
    .map(|_| ())
}

/// The same click *without* the pane wait: this click's response is being held
/// by a delayed route, so the pane must still be empty when the next step
/// clicks the second entry.
#[when(expr = "I click the entry titled {string} without waiting for the pane")]
async fn click_entry_no_wait(world: &mut RdrsWorld, title: String) -> Result<()> {
    open_entry(world, &title).await
}

async fn open_entry(world: &RdrsWorld, title: &str) -> Result<()> {
    let row = entry_row(world, title).await?;
    row.test_id("entry-title-link").await?.click().await?;
    Ok(())
}

/// The feed name inside an entry row points at the same `/feeds/{id}/entries`
/// the sidebar does, so it must take the same in-place swap rather than
/// reloading.
#[when(expr = "I click the feed name in the entry titled {string}")]
async fn click_feed_name(world: &mut RdrsWorld, title: String) -> Result<()> {
    let row = entry_row(world, &title).await?;
    row.find(thirtyfour::By::Css(".entry-feed"))
        .await?
        .click()
        .await?;
    Ok(())
}

// ── Held responses ───────────────────────────────────────────────────────────

/// Holds one entry's fragment response back, then serves it — or watches the
/// page's own stale-response guard abort it, which is the post-fix behaviour.
#[when(expr = "the fragment response for the entry titled {string} is delayed")]
async fn delay_fragment(world: &mut RdrsWorld, title: String) -> Result<()> {
    let id = entry_id(world, &title).await?;
    let handle = world
        .delay_requests(&format!(r"/entries/{id}/fragment"), HOLD)
        .await?;
    world.delayed_fragment = Some(handle);
    Ok(())
}

#[when(expr = "the fetch full content response for the entry titled {string} is delayed")]
async fn delay_full_content(world: &mut RdrsWorld, title: String) -> Result<()> {
    let id = entry_id(world, &title).await?;
    let handle = world
        .delay_requests(&format!(r"/entries/{id}/fetch-full-content"), HOLD)
        .await?;
    world.delayed_full_content = Some(handle);
    Ok(())
}

#[when("the delayed fragment response has settled")]
async fn fragment_settled(world: &mut RdrsWorld) -> Result<()> {
    let handle = world
        .delayed_fragment
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no delayed fragment route was armed"))?;
    handle.wait_for_settled(HOLD * 10).await?;
    tokio::time::sleep(SETTLE).await;
    Ok(())
}

#[then("the delayed fetch full content response has settled")]
async fn full_content_settled(world: &mut RdrsWorld) -> Result<()> {
    let handle = world
        .delayed_full_content
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no delayed full-content route was armed"))?;
    handle.wait_for_settled(HOLD * 10).await?;
    tokio::time::sleep(SETTLE).await;
    Ok(())
}

// ── Neighbour navigation ─────────────────────────────────────────────────────

fn nav_test_id(direction: &str) -> &'static str {
    if direction.eq_ignore_ascii_case("next") {
        "reading-pane-next"
    } else {
        "reading-pane-prev"
    }
}

/// The pane carries no entry id of its own, but every action form targets
/// `/entries/{id}/…` — mirrors `app.js`'s `currentPaneEntryId()` to read it.
async fn pane_entry_id(world: &RdrsWorld) -> Result<Option<String>> {
    let Some(form) = world
        .driver()?
        .css_opt(r#"#reading-pane form[action*="/entries/"]"#)
        .await?
    else {
        return Ok(None);
    };
    // The pane is mid-swap often enough that the handle goes stale between
    // finding the form and reading it; that reads as "no entry yet", which is
    // what the caller is polling for.
    let Ok(action) = form.attr("action").await else {
        return Ok(None);
    };
    let action = action.unwrap_or_default();
    Ok(rdrs_e2e::pane_entry_id_re()
        .captures(&action)
        .and_then(|caps| caps.get(1))
        .map(|id| id.as_str().to_owned()))
}

/// Captures the current entry before the click so the wait can be for the pane
/// actually swapping to a different one — which guards against a follow-up
/// navigation firing before this swap (and its neighbour re-resolve) lands.
#[when(expr = "I navigate to the {string} entry in the reading pane")]
async fn navigate_pane(world: &mut RdrsWorld, direction: String) -> Result<()> {
    let before = pane_entry_id(world).await?;
    // The button starts disabled and `app.js` enables it once
    // `/api/entries/{id}/neighbors` resolves, so this waits for clickable.
    world.driver()?.click(nav_test_id(&direction)).await?;
    eventually("the pane to show a different entry", || async {
        Ok(pane_entry_id(world).await? != before)
    })
    .await
}

#[then(expr = "the reading-pane {string} button is disabled")]
async fn pane_button_disabled(world: &mut RdrsWorld, direction: String) -> Result<()> {
    expect_enabled(world, nav_test_id(&direction), false).await
}

#[then(expr = "the reading-pane {string} button is enabled")]
async fn pane_button_enabled(world: &mut RdrsWorld, direction: String) -> Result<()> {
    expect_enabled(world, nav_test_id(&direction), true).await
}

async fn expect_enabled(world: &RdrsWorld, id: &str, expected: bool) -> Result<()> {
    let driver = world.driver()?;
    eventually(
        &format!(
            "`{id}` is {}",
            if expected { "enabled" } else { "disabled" }
        ),
        || async {
            let Some(button) = driver.test_id_opt(id).await? else {
                return Ok(false);
            };
            Ok(button.is_enabled().await? == expected)
        },
    )
    .await
}

// ── Pane contents ────────────────────────────────────────────────────────────

#[then(expr = "the reading pane shows the title {string}")]
async fn pane_title(world: &mut RdrsWorld, title: String) -> Result<()> {
    world
        .driver()?
        .expect_text("reading-pane-title", &title)
        .await
}

#[then(expr = "the reading pane shows the content {string}")]
async fn pane_content(world: &mut RdrsWorld, content: String) -> Result<()> {
    world
        .driver()?
        .expect_text("reading-pane-body", &content)
        .await
}

#[then(expr = "the reading pane shows the feed title {string}")]
async fn pane_feed_title(world: &mut RdrsWorld, title: String) -> Result<()> {
    world
        .driver()?
        .expect_text("reading-pane-feed-title", &title)
        .await
}

#[then("the reading pane shows a published time")]
async fn pane_published(world: &mut RdrsWorld) -> Result<()> {
    world
        .driver()?
        .expect_visible("reading-pane-published-at")
        .await
}

/// The favicon leads the mono dispatch eyebrow.
#[then("the reading pane shows an image favicon")]
async fn pane_favicon(world: &mut RdrsWorld) -> Result<()> {
    let favicon = world
        .driver()?
        .css(".dispatch-eyebrow img.entry-favicon")
        .await?;
    ensure!(favicon.is_displayed().await?, "the pane favicon is hidden");
    let src = favicon.attr("src").await?.unwrap_or_default();
    ensure!(
        src.ends_with("/icon"),
        "the pane favicon points at {src:?}, not an icon endpoint"
    );
    Ok(())
}

/// A synchronous snapshot right after the (delayed) navigation click: the swap
/// handler has already run `cancelPaneImages()` on the still-visible outgoing
/// pane, so the favicon's `src` reveals whether it was wrongly blanked. Read
/// once, with no retry, so it cannot be masked by the next entry eventually
/// landing.
#[then("the reading pane favicon still has its image")]
async fn pane_favicon_kept(world: &mut RdrsWorld) -> Result<()> {
    let favicon = world
        .driver()?
        .css_opt(".dispatch-eyebrow img.entry-favicon")
        .await?
        .ok_or_else(|| anyhow::anyhow!("the pane has no favicon at all"))?;
    let src = favicon.attr("src").await?.unwrap_or_default();
    ensure!(
        !src.is_empty(),
        "the reading-pane favicon lost its src during navigation"
    );
    Ok(())
}

#[then("the reading pane shows the original feed body")]
async fn pane_original_body(world: &mut RdrsWorld) -> Result<()> {
    world
        .driver()?
        .expect_attr(
            r#"[data-testid="reading-pane-body"]"#,
            "data-mode",
            Some("original"),
        )
        .await
}

#[then("the reading pane is empty")]
async fn pane_empty(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("the reading pane to be empty", || async {
        Ok(driver
            .css_opt("#reading-pane.reading-pane-empty")
            .await?
            .is_some())
    })
    .await
}

#[then("the reading pane shows a broken-image fallback")]
async fn broken_image_fallback(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    driver.css(".reading-pane-article .rp-broken-image").await?;
    let caption = driver.css(".rp-broken-cap").await?;
    ensure!(
        caption.content_text().await?.contains("Image unavailable"),
        "the broken-image caption does not say the image is unavailable"
    );
    Ok(())
}

/// The innermost `<pre>` (Rouge's gutter and code cells) must be neutralised to
/// zero padding while the outer one keeps its block padding.
#[then("the nested code-block pre has no padding while the outer pre does")]
async fn nested_pre_padding(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    let inner = driver.css_all(".reading-pane-article pre pre").await?;
    ensure!(
        !inner.is_empty(),
        "the article renders no nested <pre> at all"
    );
    let inner_pad = driver
        .computed_style(".reading-pane-article pre pre", "padding-top")
        .await?;
    let outer_pad = driver
        .computed_style(".reading-pane-article pre", "padding-top")
        .await?;
    ensure!(
        inner_pad == "0px",
        "the nested pre keeps {inner_pad} of padding"
    );
    let outer: f64 = outer_pad.trim_end_matches("px").parse().unwrap_or(0.0);
    ensure!(outer > 0.0, "the outer pre lost its padding ({outer_pad})");
    Ok(())
}

// ── Actions ──────────────────────────────────────────────────────────────────

#[when(expr = "I click {string}")]
#[when(expr = "I click the {string} button")]
async fn click_button(world: &mut RdrsWorld, label: String) -> Result<()> {
    let body = world.driver()?.css("body").await?;
    let button = body
        .button_named(&label)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the page has no `{label}` button"))?;
    click_when_ready(&button).await?;
    Ok(())
}

#[then(expr = "I see a {string} button")]
async fn see_button(world: &mut RdrsWorld, label: String) -> Result<()> {
    let body = world.driver()?.css("body").await?;
    let button = body
        .button_named(&label)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the page has no `{label}` button"))?;
    ensure!(
        button.is_displayed().await?,
        "the `{label}` button is present but hidden"
    );
    Ok(())
}

#[then(expr = "I see a {string} fetch full content action")]
async fn see_fetch_action(world: &mut RdrsWorld, label: String) -> Result<()> {
    let action = world
        .driver()?
        .css(r#"form[action*="/fetch-full-content"] button"#)
        .await?;
    let text = action.content_text().await?;
    let aria = action.attr("aria-label").await?.unwrap_or_default();
    ensure!(
        text.contains(&label) || aria.contains(&label),
        "the fetch action reads {text:?} / {aria:?}, not {label:?}"
    );
    Ok(())
}

// ── The summary panel ────────────────────────────────────────────────────────

#[then("I see the summary error banner")]
async fn see_summary_error(world: &mut RdrsWorld) -> Result<()> {
    world
        .driver()?
        .css("[data-summary-error]")
        .await
        .map(|_| ())
}

#[then("I do not see the summary error banner")]
async fn no_summary_error(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("the summary error banner to be gone", || async {
        Ok(driver.css_all("[data-summary-error]").await?.is_empty())
    })
    .await
}

#[then(expr = "I see a {string} summary action")]
async fn see_summary_action(world: &mut RdrsWorld, label: String) -> Result<()> {
    let container = world.driver()?.css("#rp-summary-container").await?;
    let button = container
        .button_named(&label)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the summary panel has no `{label}` action"))?;
    ensure!(
        button.is_displayed().await?,
        "the `{label}` summary action is hidden"
    );
    Ok(())
}

#[when(expr = "I click the {string} summary action")]
async fn click_summary_action(world: &mut RdrsWorld, label: String) -> Result<()> {
    let container = world.driver()?.css("#rp-summary-container").await?;
    let button = container
        .button_named(&label)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the summary panel has no `{label}` action"))?;
    click_when_ready(&button).await?;
    // Waits for the `#rp-summary-container` swap to settle rather than for the
    // network to go idle, which the app's background sidebar polling makes
    // unreliable.
    no_summary_error(world).await
}

// ── The address bar ──────────────────────────────────────────────────────────

/// `performSwap` rewrites the URL after a `#reading-pane` swap (`pushState` on
/// first open from an empty pane, `replaceState` on later switches), so this
/// waits until the address bar's `entry` query matches the clicked entry.
#[then(expr = "the URL has the ?entry= parameter for {string}")]
async fn url_has_entry(world: &mut RdrsWorld, title: String) -> Result<()> {
    let id = entry_id(world, &title).await?;
    eventually_eq("the ?entry= parameter", Some(id.to_string()), || async {
        entry_param(world).await
    })
    .await
}

#[then("the URL has no ?entry= parameter")]
async fn url_has_no_entry(world: &mut RdrsWorld) -> Result<()> {
    eventually_eq("the ?entry= parameter", None::<String>, || async {
        entry_param(world).await
    })
    .await
}

async fn entry_param(world: &RdrsWorld) -> Result<Option<String>> {
    let url = world.driver()?.current_url().await?;
    Ok(url
        .query_pairs()
        .find(|(key, _)| key == "entry")
        .map(|(_, value)| value.into_owned()))
}
