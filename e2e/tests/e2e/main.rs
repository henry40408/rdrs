//! The Cucumber runner (`harness = false`); run `cargo test --test e2e` from `e2e/`.
//!
//! Viewport tags (`@mobile`, `@tablet`, `@desktop`) are documentation only:
//! scenarios set their viewport through a `Given` step.

mod steps;

use cucumber::World as _;
use cucumber::writer::Stats as _;
use rdrs_e2e::Harness;
use rdrs_e2e::browser::{Browser, Scripting};
use rdrs_e2e::world::{RdrsWorld, set_pool};

/// Default features path.
const FEATURES: &str = "features";

/// Overrides [`FEATURES`] with one file or directory; cucumber's CLI is
/// unavailable because the runner owns `main`.
const FEATURES_VAR: &str = "RDRS_E2E_FEATURES";

/// Upper bound on concurrent scenarios (and browsers).
const CONCURRENCY_CEILING: usize = 4;

/// One scenario per core up to [`CONCURRENCY_CEILING`]; more browsers than
/// cores makes pages settle slower than the steps wait.
fn max_concurrent_scenarios() -> usize {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(CONCURRENCY_CEILING)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // One server per concurrent scenario so none is shared (see `world::Pool`).
    let concurrency = max_concurrent_scenarios();
    let mut servers = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        servers.push(Harness::start().await?);
    }
    set_pool(servers.iter().map(|s| s.endpoints().clone()).collect());
    // Before anything runs in parallel — see `Browser::prepare`.
    Browser::prepare().await?;

    let writer = RdrsWorld::cucumber()
        .max_concurrent_scenarios(concurrency)
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
