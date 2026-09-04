//! The step definitions, mostly one module per `steps/*.steps.js` the
//! JavaScript suite had.
//!
//! The exception is `entries.steps.js`, which at 1,000 lines covered the entry
//! list, the reading pane, the sidebar, the keyboard shortcuts and the
//! in-place-swap assertions in one file. Those are five modules here
//! ([`entries`], [`reading_pane`], [`sidebar`], [`keyboard`], [`morph`]),
//! because the file's size came from mixing them rather than from any one of
//! them being large.
//!
//! Cucumber collects `#[given]` / `#[when]` / `#[then]` attributes at link
//! time, so a module only has to be reachable from the crate root to
//! contribute its steps; nothing here is called directly.

pub mod admin;
pub mod auth;
pub mod entries;
pub mod keyboard;
pub mod morph;
pub mod onboarding;
pub mod organize;
pub mod pixel_tracking;
pub mod preferences;
pub mod pwa;
pub mod reading_pane;
pub mod responsive;
pub mod scoped_search;
pub mod search;
pub mod sidebar;
pub mod sidebar_prefs;
pub mod sse;
pub mod summarizer;
pub mod triage;
