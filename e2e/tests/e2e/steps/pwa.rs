//! The service worker: registration, offline fallback, and what the cache must
//! never contain. Browser-only; the Rust suite cannot see CSP acceptance,
//! page control or the offline fallback.

use anyhow::{Result, ensure};
use cucumber::{given, then, when};
use rdrs_e2e::dom::Dom;
use rdrs_e2e::wait::eventually;
use rdrs_e2e::world::RdrsWorld;

/// `clients.claim()` takes control without a reload, but not instantly.
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

/// Signed-in responses are `no-store` + `Vary: Cookie`, which the Cache API
/// ignores, so a cached navigation would leak one reader's articles to the next.
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

/// Driven through the form, since that value decides what goes to disk.
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
    // `offline.js` wipes on the next load before its manifest fetch, so the
    // redirect triggers it but may not have finished it.
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

/// The background's three entries.
#[given("my entries have been saved for offline reading")]
async fn entries_are_saved_offline(world: &mut RdrsWorld) -> Result<()> {
    wait_for_saved_entries(world, 3).await
}

/// For scenarios needing more entries than one list page holds.
#[given(expr = "{int} entries have been saved for offline reading")]
async fn counted_entries_are_saved_offline(world: &mut RdrsWorld, count: usize) -> Result<()> {
    wait_for_saved_entries(world, count).await
}

/// Polls the cache, not the network: a fetch that was never stored is the
/// failure worth catching.
async fn wait_for_saved_entries(world: &mut RdrsWorld, count: usize) -> Result<()> {
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
        // The library page is stored last, so it implies its fragments. `utils.js` is
        // only reachable via an `import` in `app.js`, proving the walk is transitive.
        Ok(paths.iter().any(|p| p == "/entries/offline")
            && paths.iter().any(|p| p == "/static/js/utils.js")
            && paths.iter().filter(|p| p.ends_with("/fragment")).count() == count)
    })
    .await?;

    // The asset walk scans source text, so a comment mentioning an import once
    // triggered a request; an extensionless cached path is that symptom.
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
    for path in paths.iter().filter(|p| p.starts_with("/static/")) {
        ensure!(
            path.rsplit('/')
                .next()
                .is_some_and(|name| name.contains('.') && !name.ends_with('.')),
            "{path} was saved as a static asset but does not name a file"
        );
    }
    Ok(())
}

/// `/static/` URLs requested per resource timings, which include `fetch()`.
/// CDP interception patterns cannot express "names no file".
async fn requested_static_paths(world: &mut RdrsWorld) -> Result<Vec<String>> {
    Ok(world
        .driver()?
        .execute(
            r"
            return performance.getEntriesByType('resource')
                .map((e) => new URL(e.name).pathname)
                .filter((p) => p.startsWith('/static/'));
            ",
            Vec::new(),
        )
        .await?
        .convert()?)
}

/// The asset scan cannot tell a real `url()`/`import` from a comment about
/// one, which once caused a 404 on every sync.
#[then("nothing has been asked of the server that names no file")]
async fn no_unservable_static(world: &mut RdrsWorld) -> Result<()> {
    let paths = requested_static_paths(world).await?;
    ensure!(
        !paths.is_empty(),
        "sanity: the page is expected to have loaded some static assets"
    );
    for path in &paths {
        let name = path.rsplit('/').next().unwrap_or_default();
        ensure!(
            name.contains('.') && !name.ends_with('.'),
            "the sync asked for {path}, which can name no file the server has"
        );
    }
    Ok(())
}

/// Forms (including `method="get"`), links, and auto-submitting selects.
/// Asserts all of them, since `offline.js` allowlists what *does* work.
#[then("every control that needs the server is disabled")]
async fn server_bound_controls_disabled(world: &mut RdrsWorld) -> Result<()> {
    let leaked: Vec<String> = world
        .driver()?
        .execute(
            r##"
            const WORKS_OFFLINE = 'a[data-swap="#reading-pane"], a[href="/"], a[href="/entries/offline"]';
            const SERVER_BOUND = 'form, a[href], select[data-mark-read-scope], select[data-status-select]';
            return [...document.querySelectorAll(SERVER_BOUND)]
                .filter((el) => !el.matches(WORKS_OFFLINE))
                .filter((el) => !el.hasAttribute('data-offline-disabled'))
                .map((el) => el.tagName.toLowerCase() + (el.getAttribute('action') || el.getAttribute('href') || ''));
            "##,
            Vec::new(),
        )
        .await?
        .convert()?;

    ensure!(
        leaked.is_empty(),
        "these reach the server but are still live offline: {leaked:?}"
    );
    Ok(())
}

/// Opens an entry that was never saved; `performSwap` used to fall back to a
/// real navigation, throwing the reader off the list offline. Not by title:
/// both feeds number entries from one.
#[when("I open the first entry in the list")]
async fn open_first_entry(world: &mut RdrsWorld) -> Result<()> {
    let link = world
        .driver()?
        .css(r##".entry-item a[data-swap="#reading-pane"]"##)
        .await?;
    rdrs_e2e::dom::click_when_ready(&link).await
}

#[then("Load More is disabled")]
async fn load_more_disabled(world: &mut RdrsWorld) -> Result<()> {
    world
        .driver()?
        .expect_attr("#load-more", "aria-disabled", Some("true"))
        .await
}

/// Disabling everything would be trivially correct and useless.
#[then("opening a saved entry is still offered")]
async fn opening_an_entry_still_offered(world: &mut RdrsWorld) -> Result<()> {
    let live: bool = world
        .driver()?
        .execute(
            r##"
            const link = document.querySelector('.entry-item a[data-swap="#reading-pane"]');
            return Boolean(link) && !link.hasAttribute('data-offline-disabled');
            "##,
            Vec::new(),
        )
        .await?
        .convert()?;
    ensure!(live, "no entry in the saved list can be opened");
    Ok(())
}

/// Satisfied by either `offline.js`'s pre-submit block or `performSwap`'s
/// failure message; which fires depends on `navigator.onLine`.
#[then("I am told the action has to wait for the connection")]
async fn told_the_action_must_wait(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_text("flash-message", "wait").await
}

/// The "Offline" word stays in the markup, so assert it is `display: none`.
#[then("the sidebar shows the connection is up")]
async fn sidebar_shows_connection_up(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    driver.expect_visible("connection-status").await?;
    driver.expect_hidden("connection-offline").await?;
    // The online lamp must not animate.
    let animation = driver.computed_style(".conn-dot", "animation-name").await?;
    ensure!(
        animation == "none",
        "the lamp is animated while the connection is up: {animation}"
    );
    Ok(())
}

/// Polled: offline is only known once a request fails.
#[then("the sidebar shows the connection is gone")]
async fn sidebar_shows_connection_gone(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("the connection lamp to report offline", || async {
        driver.is_visible("connection-offline").await
    })
    .await
}

/// The breath shows the app is still re-probing. Asserts a running animation,
/// not a keyframe name, to test behaviour rather than CSS.
#[then("the lamp is breathing")]
async fn lamp_is_breathing(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    let animation = driver.computed_style(".conn-dot", "animation-name").await?;
    ensure!(animation != "none", "the offline lamp sits solid");
    let cycles = driver
        .computed_style(".conn-dot", "animation-iteration-count")
        .await?;
    ensure!(
        cycles == "infinite",
        "the lamp stops breathing after {cycles} cycles, while the connection stays gone"
    );
    Ok(())
}

#[then("the sidebar offers the saved entries")]
async fn sidebar_offers_the_library(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_visible("nav-offline").await
}

#[then("the sidebar does not offer the saved entries")]
async fn sidebar_omits_the_library(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    // Wait for the self-rendered sidebar, or absence passes on an empty page.
    driver.expect_visible("nav-unread").await?;
    driver.expect_absent("nav-offline").await
}

/// With scripting on, `<noscript>` children parse as text, so read its source.
async fn scriptless_nav(world: &mut RdrsWorld) -> Result<String> {
    Ok(world
        .driver()?
        .execute(
            "return document.querySelector('noscript')?.textContent || '';",
            Vec::new(),
        )
        .await?
        .convert()?)
}

#[then("the scriptless navigation offers the saved entries")]
async fn scriptless_nav_offers_the_library(world: &mut RdrsWorld) -> Result<()> {
    let nav = scriptless_nav(world).await?;
    ensure!(
        nav.contains(r#"href="/entries/offline""#),
        "the scriptless navigation has no way to the saved entries: {nav}"
    );
    Ok(())
}

#[then("the scriptless navigation does not offer the saved entries")]
async fn scriptless_nav_omits_the_library(world: &mut RdrsWorld) -> Result<()> {
    let nav = scriptless_nav(world).await?;
    ensure!(
        !nav.contains("/entries/offline"),
        "the scriptless navigation offers a library nothing is being saved to: {nav}"
    );
    Ok(())
}

/// Offline this navigation is served from cache, so `offline.js` keeps it live.
#[when("I open the saved entries from the sidebar")]
async fn open_the_library_from_the_sidebar(world: &mut RdrsWorld) -> Result<()> {
    let link = world
        .driver()?
        .css(r#"rdrs-sidebar [data-testid="nav-offline"]"#)
        .await?;
    rdrs_e2e::dom::click_when_ready(&link).await?;
    world.expect_path("/entries/offline").await
}
