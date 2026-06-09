//! Shared helpers for the integration test suites.
//!
//! Included via `mod common;` from each `tests/*.rs` binary. Only put helpers
//! here that every (or nearly every) suite uses — per-suite `create_test_app`
//! definitions stay in their own files because each needs a unique
//! shared-memory database name to stay isolated from the other test binaries.

use rdrs::Config;

/// Default in-memory `Config` shared across the integration test suites.
pub fn default_test_config() -> Config {
    Config {
        database_url: ":memory:".to_string(),
        server_port: 3000,
        signup_enabled: true,
        multi_user_enabled: true,
        image_proxy_secret: vec![0u8; 32],
        image_proxy_secret_generated: false,
        user_agent: "RDRS-Test/1.0".to_string(),
        webauthn_rp_id: "localhost".to_string(),
        webauthn_rp_origin: "http://localhost:3000".to_string(),
        webauthn_rp_name: "rdrs-test".to_string(),
        public_base_url: None,
    }
}
