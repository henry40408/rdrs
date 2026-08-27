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
