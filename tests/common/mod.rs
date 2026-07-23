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
        server_bind: "127.0.0.1:8080".parse().unwrap(),
        signup_enabled: true,
        multi_user_enabled: true,
        image_proxy_secret: vec![0u8; 32],
        image_proxy_secret_generated: false,
        user_agent: "RDRS-Test/1.0".to_string(),
        webauthn_rp_id: "localhost".to_string(),
        webauthn_rp_origin: "http://localhost:8080".to_string(),
        webauthn_rp_name: "rdrs-test".to_string(),
        public_base_url: None,
        cookie_secure: false,
        auth_proxy_header: String::new(),
        trusted_proxy_networks: Vec::new(),
        auth_proxy_user_creation: false,
        disable_local_auth: false,
        auth_proxy_groups_header: String::new(),
        auth_proxy_admin_group: String::new(),
        auth_proxy_logout_url: None,
    }
}
