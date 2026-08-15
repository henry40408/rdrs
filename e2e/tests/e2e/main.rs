//! The Cucumber runner.
//!
//! `harness = false`: cucumber drives the scenarios itself, so there is no
//! libtest harness collecting `#[test]` functions. Run it with
//! `cargo test --test e2e` from `e2e/`.
//!
//! What `playwright.config.js` expressed as a project plus worker-scoped
//! fixtures is expressed here as one server started up front and a `before`
//! hook that opens a session per scenario. The viewport tags (`@mobile`,
//! `@tablet`, `@desktop`) are documentation: the scenarios set their own
//! viewport through a `Given` step, as they did under Playwright.

mod steps;

use cucumber::World as _;
use cucumber::writer::Stats as _;
use rdrs_e2e::Harness;
use rdrs_e2e::browser::{Browser, Scripting};
use rdrs_e2e::world::{RdrsWorld, set_endpoints};

/// Where the `.feature` files live, and what a run covers by default.
const FEATURES: &str = "features";

/// Overrides [`FEATURES`] with a single file or directory.
///
/// Stands in for `npx playwright test --grep`, which is how one feature was
/// run while working on it. Cucumber's own CLI is not reachable here — the
/// runner owns `main` so it can start the server first.
const FEATURES_VAR: &str = "RDRS_E2E_FEATURES";

/// The most scenarios — and so browsers — to run at once, whatever the machine.
const CONCURRENCY_CEILING: usize = 4;

/// How many scenarios run at once, one per core up to [`CONCURRENCY_CEILING`].
///
/// A fixed four was wrong in the sibling project this was ported from: fine on
/// a developer's machine and too many for a two-core CI runner, where four
/// browsers contend for two cores until pages take longer to settle than the
/// steps wait for.
fn max_concurrent_scenarios() -> usize {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(CONCURRENCY_CEILING)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Killed when this binding drops at the end of `main`.
    let harness = Harness::start().await?;
    set_endpoints(harness.endpoints().clone());
    // Before anything runs in parallel — see `Browser::prepare`.
    Browser::prepare().await?;

    let writer = RdrsWorld::cucumber()
        // Each scenario gets its own browser and its own account, so they do
        // not interfere. `Browser::prepare` must have run first.
        .max_concurrent_scenarios(max_concurrent_scenarios())
        .fail_on_skipped()
        .before(|_feature, _rule, _scenario, world| {
            Box::pin(async move {
                world
                    .open(Scripting::Enabled)
                    .await
                    .expect("could not open a browser session");
            })
        })
        .after(|_feature, _rule, _scenario, _finished, world| {
            Box::pin(async move {
                if let Some(world) = world {
                    world.close().await.expect("could not close the session");
                }
            })
        })
        .run(std::env::var(FEATURES_VAR).unwrap_or_else(|_| FEATURES.to_owned()))
        .await;

    let failures = writer.failed_steps() + writer.parsing_errors() + writer.hook_errors();
    anyhow::ensure!(failures == 0, "{failures} cucumber failure(s)");
    Ok(())
}
