//! The service worker: registration, the offline fallback, and the one thing
//! the cache must never contain.
//!
//! All of it is browser-only. The Rust suite can prove `/sw.js`, the manifest
//! and `/offline` are served correctly, but not that a browser accepts the
//! worker under the app's own CSP, takes control of the page, or reaches the
//! fallback when the network is gone.

use anyhow::{Result, ensure};
use cucumber::{given, then, when};
use rdrs_e2e::dom::Dom;
use rdrs_e2e::wait::eventually;
use rdrs_e2e::world::RdrsWorld;

/// `pwa.js` registers on `load` and the worker calls `clients.claim()`, so the
/// page it was registered from becomes controlled without a reload — but not
/// instantly, hence the poll.
#[given("a service worker controls the page")]
#[then("a service worker controls the page")]
async fn service_worker_controls_the_page(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("a service worker to take control", || async {
        let controlled = driver
            .execute(
                "return Boolean(navigator.serviceWorker && navigator.serviceWorker.controller);",
                Vec::new(),
            )
            .await?;
        Ok(controlled.json().as_bool().unwrap_or(false))
    })
    .await
}

#[when("the network goes offline")]
async fn network_goes_offline(world: &mut RdrsWorld) -> Result<()> {
    world.browser()?.set_offline(true).await
}

#[then("I see the offline page")]
async fn see_the_offline_page(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_visible("offline-page").await
}

/// The invariant the whole design rests on. Every signed-in response is
/// `no-store` + `Vary: Cookie` and the Cache API honours neither, so a worker
/// that cached a navigation would leave one reader's articles on disk for
/// whoever opens the app next. Reading the cache back is the only way to see
/// that it did not.
#[then("the worker's cache holds nothing but public assets")]
async fn cache_holds_only_public_assets(world: &mut RdrsWorld) -> Result<()> {
    let cached: Vec<String> = world
        .driver()?
        .execute_async(
            r"
            const done = arguments[arguments.length - 1];
            (async () => {
                const paths = [];
                for (const name of await caches.keys()) {
                    const cache = await caches.open(name);
                    for (const request of await cache.keys()) {
                        paths.push(new URL(request.url).pathname);
                    }
                }
                done(paths);
            })();
            ",
            Vec::new(),
        )
        .await?
        .convert()?;

    ensure!(
        !cached.is_empty(),
        "sanity: the worker is expected to have precached something"
    );
    for path in &cached {
        ensure!(
            path == "/offline" || path.starts_with("/static/"),
            "the cache holds {path}, which is neither the offline page nor a public asset: {cached:?}"
        );
    }
    Ok(())
}

/// The preferences form, driven rather than seeded: the number the reader types
/// is what decides whether anything of theirs goes to disk at all, so the path
/// that sets it is worth exercising.
async fn set_offline_keep(world: &mut RdrsWorld, keep: &str) -> Result<()> {
    world.goto("/user-settings").await?;
    let driver = world.driver()?;
    driver.expect_visible("offline-keep").await?;
    driver.fill("offline-keep", keep).await?;
    driver
        .submit_css(r#"form[action="/user-settings/preferences"] button[type=submit]"#)
        .await?;
    world.expect_path("/user-settings").await
}

#[given(expr = "I keep {int} entries for offline reading")]
async fn keep_entries_offline(world: &mut RdrsWorld, keep: u32) -> Result<()> {
    set_offline_keep(world, &keep.to_string()).await
}

#[when("I stop keeping entries for offline reading")]
async fn stop_keeping_entries_offline(world: &mut RdrsWorld) -> Result<()> {
    set_offline_keep(world, "0").await?;
    // The wipe is `offline.js`'s first act on the next load, before its manifest
    // fetch — so the redirect above is enough to have triggered it, but not to
    // have finished it.
    let driver = world.driver()?;
    eventually("the saved entries to be dropped", || async {
        let names: Vec<String> = driver
            .execute_async(
                r"
                const done = arguments[arguments.length - 1];
                caches.keys().then((names) => done(names.filter((n) => n.startsWith('rdrs-offline-'))));
                ",
                Vec::new(),
            )
            .await?
            .convert()?;
        Ok(names.is_empty())
    })
    .await
}

/// Waits for the sync to have mirrored the queue. Polls the cache rather than
/// the network, because "saved" is a statement about what is on disk — a
/// completed fetch that was never stored is exactly the failure worth catching.
#[given("my entries have been saved for offline reading")]
async fn entries_are_saved_offline(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("the entry list to be mirrored into the offline cache", || async {
        let paths: Vec<String> = driver
            .execute_async(
                r"
                const done = arguments[arguments.length - 1];
                (async () => {
                    const names = (await caches.keys()).filter((n) => n.startsWith('rdrs-offline-'));
                    const paths = [];
                    for (const name of names) {
                        const cache = await caches.open(name);
                        for (const request of await cache.keys()) {
                            paths.push(new URL(request.url).pathname);
                        }
                    }
                    done(paths);
                })();
                ",
                Vec::new(),
            )
            .await?
            .convert()?;
        // The library page is stored last, so its presence means the fragments
        // it lists are already there. `utils.js` is named by no document — only
        // by an `import` inside `app.js` — so requiring it is what proves the
        // asset walk is transitive rather than a scrape of the current page.
        Ok(paths.iter().any(|p| p == "/entries/offline")
            && paths.iter().any(|p| p == "/static/js/utils.js")
            && paths.iter().filter(|p| p.ends_with("/fragment")).count() == 3)
    })
    .await
}

#[when("I try to mark the open entry unread")]
async fn try_to_mark_unread(world: &mut RdrsWorld) -> Result<()> {
    world
        .driver()?
        .click_css(r#"#reading-pane form[action$="/unread"] button"#)
        .await
}

/// Either half of the offline story satisfies this: `offline.js` blocks the
/// submit outright when the browser reports no connection, and `performSwap`
/// says the same thing when the request it sent anyway never came back. Which
/// one fires depends on whether the browser updated `navigator.onLine`, which
/// is exactly the judgement this feature refuses to depend on.
#[then("I am told the action has to wait for the connection")]
async fn told_the_action_must_wait(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_text("flash-message", "wait").await
}
