use url::Url;
use webauthn_rs::prelude::*;

use crate::config::Config;
use crate::error::{AppError, AppResult};

pub fn create_webauthn(config: &Config) -> AppResult<Webauthn> {
    let rp_origin =
        Url::parse(&config.webauthn_rp_origin).map_err(|e| AppError::Internal(e.to_string()))?;

    let builder = WebauthnBuilder::new(&config.webauthn_rp_id, &rp_origin)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .rp_name(&config.webauthn_rp_name);

    builder
        .build()
        .map_err(|e| AppError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            database_url: "test.db".to_string(),
            server_bind: "127.0.0.1:8080".parse().unwrap(),
            signup_enabled: true,
            multi_user_enabled: false,
            secret: vec![0u8; 32],
            secret_generated: false,
            user_agent: "test".to_string(),
            webauthn_rp_id: "localhost".to_string(),
            webauthn_rp_origin: "http://localhost:8080".to_string(),
            webauthn_rp_name: "rdrs".to_string(),
            public_base_url: None,
            cookie_secure: false,
            auth_proxy_header: String::new(),
            trusted_proxy_networks: Vec::new(),
            auth_proxy_user_creation: false,
            disable_local_auth: false,
            auth_proxy_groups_header: String::new(),
            auth_proxy_admin_group: String::new(),
            auth_proxy_logout_url: None,
            login_rate_limit_attempts: crate::middleware::rate_limit::LOGIN_MAX_ATTEMPTS,
            login_rate_limit_window_secs: crate::middleware::rate_limit::LOGIN_WINDOW_SECS,
            hsts: false,
            hsts_max_age: 31_536_000,
            hsts_include_subdomains: true,
            greader_legacy_session_tokens: false,
        }
    }

    #[test]
    fn test_create_webauthn() {
        let config = test_config();
        let webauthn = create_webauthn(&config);
        assert!(webauthn.is_ok());
    }

    #[test]
    fn test_create_webauthn_invalid_origin() {
        let mut config = test_config();
        config.webauthn_rp_origin = "not-a-valid-url".to_string();
        let webauthn = create_webauthn(&config);
        assert!(webauthn.is_err());
    }
}
