use base64::{engine::general_purpose::STANDARD, Engine};
use rand::Rng;
use std::env;

/// Default user agent for HTTP requests (transparent and responsible crawling)
pub const DEFAULT_USER_AGENT: &str = concat!(
    "RDRS/",
    env!("GIT_VERSION"),
    " (RSS Reader; +https://github.com/henry40408/rdrs)"
);

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub server_port: u16,
    pub signup_enabled: bool,
    pub multi_user_enabled: bool,
    pub image_proxy_secret: Vec<u8>,
    pub image_proxy_secret_generated: bool,
    pub user_agent: String,
    pub webauthn_rp_id: String,
    pub webauthn_rp_origin: String,
    pub webauthn_rp_name: String,
    pub public_base_url: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        let (image_proxy_secret, image_proxy_secret_generated) = Self::load_image_proxy_secret();
        let server_port = env::var("SERVER_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3000);

        Self {
            database_url: env::var("DATABASE_URL").unwrap_or_else(|_| "rdrs.sqlite3".to_string()),
            server_port,
            signup_enabled: env::var("SIGNUP_ENABLED")
                .map(|v| v.to_lowercase() == "true" || v == "1")
                .unwrap_or(false),
            multi_user_enabled: env::var("MULTI_USER_ENABLED")
                .map(|v| v.to_lowercase() == "true" || v == "1")
                .unwrap_or(false),
            image_proxy_secret,
            image_proxy_secret_generated,
            user_agent: env::var("USER_AGENT").unwrap_or_else(|_| DEFAULT_USER_AGENT.to_string()),
            webauthn_rp_id: env::var("WEBAUTHN_RP_ID").unwrap_or_else(|_| "localhost".to_string()),
            webauthn_rp_origin: env::var("WEBAUTHN_RP_ORIGIN")
                .unwrap_or_else(|_| format!("http://localhost:{}", server_port)),
            webauthn_rp_name: env::var("WEBAUTHN_RP_NAME").unwrap_or_else(|_| "rdrs".to_string()),
            public_base_url: env::var("PUBLIC_BASE_URL").ok().filter(|s| !s.is_empty()),
        }
    }

    fn load_image_proxy_secret() -> (Vec<u8>, bool) {
        if let Ok(secret_str) = env::var("IMAGE_PROXY_SECRET") {
            // Try to decode as base64 first
            if let Ok(decoded) = STANDARD.decode(&secret_str) {
                if decoded.len() >= 16 {
                    return (decoded, false);
                }
            }
            // Use raw bytes if at least 16 characters
            if secret_str.len() >= 16 {
                return (secret_str.into_bytes(), false);
            }
        }

        // Generate a random 32-byte secret
        let mut secret = vec![0u8; 32];
        rand::rng().fill_bytes(&mut secret);
        (secret, true)
    }

    /// Whether a new account may be registered given the current user count.
    ///
    /// The very first account is always allowed so a fresh install (including a
    /// source build that never set `SIGNUP_ENABLED`) can always create its
    /// initial admin. Every subsequent account requires both `SIGNUP_ENABLED`
    /// and `MULTI_USER_ENABLED`.
    pub fn can_register(&self, user_count: i64) -> bool {
        user_count == 0 || (self.signup_enabled && self.multi_user_enabled)
    }

    /// A startup warning about WebAuthn relying-party config that would silently
    /// break passkeys in a real deployment, or `None` when the config looks
    /// deployable. Returns a message when the RP origin still points at
    /// `localhost` (the default), or when it disagrees with `PUBLIC_BASE_URL`.
    pub fn webauthn_rp_warning(&self) -> Option<String> {
        if self.webauthn_rp_origin.contains("localhost") {
            return Some(format!(
                "WEBAUTHN_RP_ORIGIN is still '{}'. Passkeys will be rejected from any other \
                 origin — set WEBAUTHN_RP_ID and WEBAUTHN_RP_ORIGIN to your deployment domain.",
                self.webauthn_rp_origin
            ));
        }
        if let Some(base) = &self.public_base_url {
            if base.trim_end_matches('/') != self.webauthn_rp_origin.trim_end_matches('/') {
                return Some(format!(
                    "WEBAUTHN_RP_ORIGIN ('{}') does not match PUBLIC_BASE_URL ('{}'). Passkeys \
                     may be rejected — align WEBAUTHN_RP_ORIGIN with the URL users access.",
                    self.webauthn_rp_origin, base
                ));
            }
        }
        None
    }
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
            user_agent: DEFAULT_USER_AGENT.to_string(),
            webauthn_rp_id: "localhost".to_string(),
            webauthn_rp_origin: "http://localhost:3000".to_string(),
            webauthn_rp_name: "rdrs".to_string(),
            public_base_url: None,
        }
    }

    #[test]
    fn test_can_register() {
        // Single-user (signup on, multi-user off): only the first account.
        let config = test_config();
        assert!(config.can_register(0));
        assert!(!config.can_register(1));

        // Multi-user: every account is allowed.
        let config_multi = Config {
            multi_user_enabled: true,
            ..config.clone()
        };
        assert!(config_multi.can_register(0));
        assert!(config_multi.can_register(5));

        // Signup disabled: the first account still works (fresh-install bootstrap),
        // but no further accounts may register.
        let config_disabled = Config {
            signup_enabled: false,
            ..config
        };
        assert!(config_disabled.can_register(0));
        assert!(!config_disabled.can_register(1));
    }

    #[test]
    fn test_webauthn_rp_warning() {
        // Default localhost origin → warn.
        let config = test_config();
        assert!(config.webauthn_rp_warning().is_some());

        // Real domain, no PUBLIC_BASE_URL → fine.
        let deployed = Config {
            webauthn_rp_id: "rdrs.example.com".to_string(),
            webauthn_rp_origin: "https://rdrs.example.com".to_string(),
            ..test_config()
        };
        assert!(deployed.webauthn_rp_warning().is_none());

        // Real domain matching PUBLIC_BASE_URL (trailing slash ignored) → fine.
        let matched = Config {
            public_base_url: Some("https://rdrs.example.com/".to_string()),
            ..deployed.clone()
        };
        assert!(matched.webauthn_rp_warning().is_none());

        // Real domain disagreeing with PUBLIC_BASE_URL → warn.
        let mismatched = Config {
            public_base_url: Some("https://reader.example.com".to_string()),
            ..deployed
        };
        assert!(mismatched.webauthn_rp_warning().is_some());
    }
}
