//! Step definitions, one module per area. Cucumber collects the step
//! attributes at link time, so a module only needs to be reachable.

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
