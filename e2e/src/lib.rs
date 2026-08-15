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

/// Matches the one-time invite path inside a flash message.
pub fn invite_path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"/invite/[A-Za-z0-9_-]+").expect("the invite pattern compiles"))
}
