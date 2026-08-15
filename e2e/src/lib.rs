//! Browser E2E support for rdrs: the server under test, the throwaway accounts
//! the scenarios run as, the seed helper that fills their databases, and the
//! browser session the Cucumber steps drive.

use std::sync::OnceLock;

use rand::RngExt;
use regex::Regex;

pub mod api;
pub mod browser;
pub mod dom;
pub mod network;
pub mod seed;
pub mod server;
pub mod wait;
pub mod world;

pub use server::Harness;

/// Eight lowercase alphanumerics, the shape `nanoid` produced for usernames.
///
/// Accounts are never cleaned up — one server serves the whole run — so every
/// scenario needs a name no other scenario will pick.
pub fn random_slug() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..8)
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect()
}

/// The first cell of every row in a step's data table.
///
/// Every table in this suite is a single column — titles, URLs — which is what
/// `table.raw().map((row) => row[0])` said in the JavaScript steps.
///
/// # Errors
///
/// Fails when the step carries no table.
pub fn first_column(step: &cucumber::gherkin::Step) -> anyhow::Result<Vec<String>> {
    let table = step
        .table
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("this step needs a data table"))?;
    Ok(table
        .rows
        .iter()
        .filter_map(|row| row.first().cloned())
        .collect())
}

/// Matches the one-time invite path inside a flash message.
pub fn invite_path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"/invite/[A-Za-z0-9_-]+").expect("the invite pattern compiles"))
}

/// Matches a `HH:MM:SS` clock reading, the shape of the flash timestamp.
pub fn clock_time_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d{2}:\d{2}:\d{2}$").expect("the clock pattern compiles"))
}

/// Captures the entry id out of a reading-pane action form's `action`.
pub fn pane_entry_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"/entries/(\d+)/").expect("the pane id pattern compiles"))
}

/// Matches the URL a feed's stored favicon is served from.
pub fn feed_icon_src_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"/api/feeds/\d+/icon").expect("the icon pattern compiles"))
}
