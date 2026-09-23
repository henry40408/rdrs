//! Live updates over SSE; every assertion polls the DOM without navigating.

use anyhow::Result;
use cucumber::{then, when};
use rdrs_e2e::dom::{Dom, TextContent};
use rdrs_e2e::wait::{eventually, eventually_eq};
use rdrs_e2e::world::RdrsWorld;

/// The mock Kagi upstream is slow so the transient badge is observable.
#[then("the entry row shows a pending summary badge")]
async fn pending_badge(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("a pending or processing summary badge", || async {
        let badge = driver
            .css_opt(".summary-badge-pending, .summary-badge-processing")
            .await?;
        match badge {
            Some(element) => Ok(element.is_displayed().await.unwrap_or(false)),
            None => Ok(false),
        }
    })
    .await
}

/// The SSE event swaps `#rp-summary-container` in place.
#[then("without reloading, the reading pane shows the completed summary")]
async fn completed_summary_in_pane(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("the reading pane's completed summary", || async {
        let Some(container) = driver.css_opt("#rp-summary-container").await? else {
            return Ok(false);
        };
        Ok(container
            .content_text()
            .await?
            .contains("E2E mock summary body."))
    })
    .await
}

#[then("the entry row shows the completed summary badge")]
async fn completed_badge(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("the completed summary badge", || async {
        let badge = driver
            .css_opt(r#".summary-badge[title="Has Summary"]"#)
            .await?;
        match badge {
            Some(element) => Ok(element.is_displayed().await.unwrap_or(false)),
            None => Ok(false),
        }
    })
    .await
}

/// Snapshots the unread count and marks read in one step, so the snapshot
/// precedes the SSE event.
#[when(expr = "a background request marks {string} as read")]
async fn background_mark_read(world: &mut RdrsWorld, title: String) -> Result<()> {
    world.unread_before = Some(unread_count(world).await?);

    let user_id = world.user_id().await?;
    let entry_id = world.seed().entry_id_by_title(user_id, &title).await?;
    world
        .post_as_user(&format!("/entries/{entry_id}/read"))
        .await
}

/// The SSE event triggers `rdrs-sidebar.refresh()`, updating the badge in place.
#[then("within 5 seconds the sidebar unread count decreases by one without a reload")]
async fn unread_decreases(world: &mut RdrsWorld) -> Result<()> {
    let before = world
        .unread_before
        .ok_or_else(|| anyhow::anyhow!("the unread count was not captured in the When step"))?;
    let expected = before.saturating_sub(1);
    eventually_eq("the sidebar unread count", expected, || unread_count(world)).await
}

async fn unread_count(world: &RdrsWorld) -> Result<u32> {
    let Some(element) = world.driver()?.css_opt("#unread-count").await? else {
        return Ok(0);
    };
    Ok(element.content_text().await?.trim().parse().unwrap_or(0))
}
