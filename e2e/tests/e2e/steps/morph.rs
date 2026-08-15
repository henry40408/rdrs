//! The in-place-swap assertions: tag the DOM, do something, and check that what
//! is still tagged is everything that should not have been rebuilt.
//!
//! Split out of `entries.steps.js` — see [`super::entries`].
//!
//! Every tag here is a **JS property**, not a `data-` attribute. An attribute
//! would change the node's `outerHTML`, and `outerHTML` is what `performSwap`
//! compares to decide a row fragment is unchanged — so tagging by attribute
//! would defeat the very skip these scenarios assert. (The sidebar's own
//! tagging in [`super::sidebar`] does use attributes: that path reconciles
//! rather than comparing markup.)

use anyhow::{Result, ensure};
use cucumber::{then, when};
use rdrs_e2e::dom::Dom;
use rdrs_e2e::wait::eventually;
use rdrs_e2e::world::RdrsWorld;

#[when("I tag the entry rows")]
async fn tag_entry_rows(world: &mut RdrsWorld) -> Result<()> {
    let tagged = world
        .driver()?
        .eval(
            r#"
            const nodes = document.querySelectorAll("[data-entry-row], [data-entry-row] *");
            for (const node of nodes) node.__e2eTag = true;
            return nodes.length;
            "#,
        )
        .await?;
    ensure!(
        tagged.as_u64().unwrap_or(0) > 0,
        "there were no entry rows to tag"
    );
    Ok(())
}

#[then("the entry rows are still the ones I tagged")]
async fn entry_rows_still_tagged(world: &mut RdrsWorld) -> Result<()> {
    let untagged = world
        .driver()?
        .eval(
            r#"
            return [...document.querySelectorAll("[data-entry-row], [data-entry-row] *")]
              .filter((node) => !node.__e2eTag)
              .map((node) =>
                `${node.nodeName.toLowerCase()}.${node.className} in #${
                  node.closest("[data-entry-row]")?.id
                }`);
            "#,
        )
        .await?;
    let rebuilt = untagged.as_array().map(Vec::as_slice).unwrap_or_default();
    ensure!(
        rebuilt.is_empty(),
        "these nodes were rebuilt rather than left alone: {rebuilt:?}"
    );
    Ok(())
}

#[when("I tag the entry list contents")]
async fn tag_list_contents(world: &mut RdrsWorld) -> Result<()> {
    let counts = world
        .driver()?
        .eval(
            r#"
            const list = document.querySelector("[data-entries-list]");
            const rows = [...list.querySelectorAll("[data-entry-row]")];
            const icons = [...list.querySelectorAll("img.entry-favicon")];
            list.__e2eMorphTag = true;
            for (const node of [...rows, ...icons]) node.__e2eMorphTag = true;
            return { rows: rows.length, icons: icons.length };
            "#,
        )
        .await?;
    ensure!(
        counts["rows"].as_u64().unwrap_or(0) > 0,
        "the list had no rows to tag"
    );
    ensure!(
        counts["icons"].as_u64().unwrap_or(0) > 0,
        "the list had no favicons to tag"
    );
    Ok(())
}

#[then("the entry list contents are still the ones I tagged")]
async fn list_contents_still_tagged(world: &mut RdrsWorld) -> Result<()> {
    let kept = world
        .driver()?
        .eval(
            r#"
            const list = document.querySelector("[data-entries-list]");
            const nodes = [
              ...list.querySelectorAll("[data-entry-row]"),
              ...list.querySelectorAll("img.entry-favicon"),
            ];
            return {
              container: list.__e2eMorphTag === true,
              total: nodes.length,
              tagged: nodes.filter((n) => n.__e2eMorphTag).length,
            };
            "#,
        )
        .await?;
    ensure!(
        kept["container"].as_bool().unwrap_or(false),
        "the list container itself was replaced"
    );
    let total = kept["total"].as_u64().unwrap_or(0);
    let tagged = kept["tagged"].as_u64().unwrap_or(0);
    ensure!(total > 0, "the list is empty after the interaction");
    ensure!(
        tagged == total,
        "{} of {total} nodes were rebuilt",
        total - tagged
    );
    Ok(())
}

/// Arms the check for the *next* list-pane render before the click that causes
/// it, so the assertion can tell a landed response from a guess.
#[when("I tag the entry list pane")]
async fn tag_list_pane(world: &mut RdrsWorld) -> Result<()> {
    let state = world
        .driver()?
        .eval(
            r#"
            const pane = document.querySelector("[data-list-pane]");
            pane.__e2ePaneTag = true;
            const rows = [...pane.querySelectorAll("[data-entry-row]")];
            const icons = [...pane.querySelectorAll("img")];
            for (const node of [...rows, ...icons]) node.__e2ePaneTag = true;
            return {
              rows: rows.length,
              icons: icons.length,
              stamp: pane.querySelector("[data-snapshot-at]")?.getAttribute("data-snapshot-at"),
            };
            "#,
        )
        .await?;
    ensure!(
        state["rows"].as_u64().unwrap_or(0) > 0,
        "the pane had no rows to tag"
    );
    ensure!(
        state["icons"].as_u64().unwrap_or(0) > 0,
        "the pane had no images to tag"
    );
    world.pane_stamp = state["stamp"].as_str().map(str::to_owned);
    Ok(())
}

#[then("the entry list pane is still the one I tagged")]
async fn pane_still_tagged(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    // Two frames for the swap logic that runs on the response to have its say.
    driver
        .eval("return new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));")
        .await?;
    let kept = driver
        .eval(
            r#"
            const pane = document.querySelector("[data-list-pane]");
            const nodes = [
              ...pane.querySelectorAll("[data-entry-row]"),
              ...pane.querySelectorAll("img"),
            ];
            return {
              pane: pane.__e2ePaneTag === true,
              total: nodes.length,
              tagged: nodes.filter((n) => n.__e2ePaneTag).length,
            };
            "#,
        )
        .await?;
    ensure!(
        kept["pane"].as_bool().unwrap_or(false),
        "the list pane itself was replaced"
    );
    let total = kept["total"].as_u64().unwrap_or(0);
    let tagged = kept["tagged"].as_u64().unwrap_or(0);
    ensure!(total > 0, "the pane is empty after the interaction");
    ensure!(
        tagged == total,
        "{} of {total} nodes were rebuilt",
        total - tagged
    );
    Ok(())
}

/// `data-snapshot-at` has one-second resolution, so two renders inside the same
/// second carry the same stamp and "did it advance?" would be unanswerable.
#[when("I let the render stamp age")]
async fn age_render_stamp(_world: &mut RdrsWorld) -> Result<()> {
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    Ok(())
}

#[then("the list's render stamp has advanced")]
async fn stamp_advanced(world: &mut RdrsWorld) -> Result<()> {
    let before = world
        .pane_stamp
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no render stamp was captured"))?;
    let driver = world.driver()?;
    eventually("the render stamp to advance", || async {
        let stamp = driver
            .eval(
                r#"return document.querySelector("[data-list-pane] [data-snapshot-at]")
                     ?.getAttribute("data-snapshot-at");"#,
            )
            .await?;
        Ok(stamp.as_str() != Some(before.as_str()))
    })
    .await
}

/// A document load wipes anything hung off `window`, so a marker set before the
/// interaction and still readable after it proves the switch stayed in the same
/// document — which is the whole point of the list-pane swap, since a reload
/// resets the sidebar's own scroll offset.
#[when("I mark the document for reload detection")]
async fn mark_document(world: &mut RdrsWorld) -> Result<()> {
    world
        .driver()?
        .eval("window.__rdrsDocumentMarker = true; return true;")
        .await?;
    Ok(())
}

#[then("the document did not reload")]
async fn document_did_not_reload(world: &mut RdrsWorld) -> Result<()> {
    let marker = world
        .driver()?
        .eval("return window.__rdrsDocumentMarker === true;")
        .await?;
    ensure!(
        marker.as_bool().unwrap_or(false),
        "the marker is gone, so the document reloaded"
    );
    Ok(())
}

/// Waits for every favicon in the list to have decoded, so a later assertion
/// about the images is not racing their load.
#[when("the entry list favicons have loaded")]
async fn favicons_loaded(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    driver.css("[data-entries-list] img.entry-favicon").await?;
    eventually("every list favicon to finish loading", || async {
        let loaded = driver
            .eval(
                r#"
                const icons = [...document.querySelectorAll("[data-entries-list] img.entry-favicon")];
                return icons.length > 0 && icons.every((i) => i.complete && i.naturalWidth > 0);
                "#,
            )
            .await?;
        Ok(loaded.as_bool().unwrap_or(false))
    })
    .await
}
