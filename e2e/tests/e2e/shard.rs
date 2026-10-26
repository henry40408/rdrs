//! Splits the suite across CI jobs: `RDRS_E2E_SHARD=<index>/<total>` (1-based) keeps
//! only the scenarios that hash into that shard. Unset, everything runs.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as _, Hasher as _};
use std::sync::atomic::{AtomicUsize, Ordering};

use cucumber::gherkin;

const SHARD_VAR: &str = "RDRS_E2E_SHARD";

static SELECTED: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
pub struct Shard {
    index: u64,
    total: u64,
}

impl Shard {
    /// `None` when `RDRS_E2E_SHARD` is unset; a malformed value is an error rather
    /// than a silent full run, which would hide a broken CI matrix.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Ok(value) = std::env::var(SHARD_VAR) else {
            return Ok(None);
        };
        let parsed = value.split_once('/').and_then(|(index, total)| {
            Some((index.parse::<u64>().ok()?, total.parse::<u64>().ok()?))
        });
        match parsed {
            Some((index, total)) if (1..=total).contains(&index) => Ok(Some(Self { index, total })),
            _ => anyhow::bail!("{SHARD_VAR} must look like `2/3`, got `{value}`"),
        }
    }

    /// The same binary runs in every shard, so `DefaultHasher` (fixed keys)
    /// puts a given scenario in the same shard everywhere. The line number
    /// tells apart the rows an outline expands into.
    pub fn includes(self, feature: &gherkin::Feature, scenario: &gherkin::Scenario) -> bool {
        let mut hasher = DefaultHasher::new();
        feature.name.hash(&mut hasher);
        scenario.name.hash(&mut hasher);
        scenario.position.line.hash(&mut hasher);
        let selected = hasher.finish() % self.total + 1 == self.index;
        if selected {
            SELECTED.fetch_add(1, Ordering::Relaxed);
        }
        selected
    }
}

/// A shard that selected nothing means the split is wrong, and a green run
/// would silently skip those scenarios.
pub fn ensure_selected(shard: Option<Shard>) -> anyhow::Result<()> {
    let Some(shard) = shard else {
        return Ok(());
    };
    let selected = SELECTED.load(Ordering::Relaxed);
    println!(
        "shard {}/{}: {selected} scenarios",
        shard.index, shard.total
    );
    anyhow::ensure!(
        selected > 0,
        "shard {}/{} selected no scenarios",
        shard.index,
        shard.total
    );
    Ok(())
}
