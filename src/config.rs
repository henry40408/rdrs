use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub server_port: u16,
    pub signup_enabled: bool,
    pub multi_user_enabled: bool,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            database_url: env::var("DATABASE_URL").unwrap_or_else(|_| "rdrs.sqlite3".to_string()),
            server_port: env::var("SERVER_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3000),
            signup_enabled: env::var("SIGNUP_ENABLED")
                .map(|v| v.to_lowercase() == "true" || v == "1")
                .unwrap_or(false),
            multi_user_enabled: env::var("MULTI_USER_ENABLED")
                .map(|v| v.to_lowercase() == "true" || v == "1")
                .unwrap_or(false),
        }
    }

    pub fn can_register(&self, user_count: i64) -> bool {
        self.signup_enabled && (self.multi_user_enabled || user_count == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_register() {
        let config = Config {
            database_url: "test.db".to_string(),
            server_port: 3000,
            signup_enabled: true,
            multi_user_enabled: false,
        };

        assert!(config.can_register(0));
        assert!(!config.can_register(1));

        let config_multi = Config {
            multi_user_enabled: true,
            ..config.clone()
        };
        assert!(config_multi.can_register(0));
        assert!(config_multi.can_register(5));

        let config_disabled = Config {
            signup_enabled: false,
            ..config
        };
        assert!(!config_disabled.can_register(0));
    }
}
