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

    builder.build().map_err(|e| AppError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            database_url: "test.db".to_string(),
            server_port: 3000,
            signup_enabled: true,
            multi_user_enabled: false,
            image_proxy_secret: vec![0u8; 32],
            image_proxy_secret_generated: false,
            user_agent: "test".to_string(),
            webauthn_rp_id: "localhost".to_string(),
            webauthn_rp_origin: "http://localhost:3000".to_string(),
            webauthn_rp_name: "rdrs".to_string(),
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
