pub mod linkding;

use serde::{Deserialize, Serialize};

pub use linkding::LinkdingConfig;

use super::summarize::KagiConfig;

#[derive(Debug, Clone)]
pub struct BookmarkData {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

/// The outcome of one service's save, rendered per-service in the UI.
#[derive(Debug, Clone, Serialize)]
pub struct SaveResult {
    pub success: bool,
    pub service: String,
    pub message: String,
    pub bookmark_url: Option<String>,
}

/// Configuration for all save services (stored as JSON in database)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SaveServicesConfig {
    #[serde(default)]
    pub linkding: Option<LinkdingConfig>,
    #[serde(default)]
    pub kagi: Option<KagiConfig>,
    // Future services can be added here:
    // pub pocket: Option<PocketConfig>,
    // pub wallabag: Option<WallabagConfig>,
}

impl SaveServicesConfig {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Only services whose credentials are actually filled in — a present but
    /// half-configured entry does not count.
    pub fn configured_services(&self) -> Vec<&'static str> {
        let mut services = Vec::new();
        if self
            .linkding
            .as_ref()
            .is_some_and(linkding::LinkdingConfig::is_configured)
        {
            services.push("linkding");
        }
        // Add more services here as they are implemented
        services
    }

    pub fn has_any_service(&self) -> bool {
        !self.configured_services().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_services_config_json_roundtrip() {
        let json = r#"{"linkding":{"api_url":"https://l","api_token":"t"}}"#;
        let cfg = SaveServicesConfig::from_json(json).unwrap();
        assert!(cfg.has_any_service());
        let back = cfg.to_json().unwrap();
        assert!(back.contains("https://l"));
    }

    #[test]
    fn empty_config_has_no_service() {
        let cfg = SaveServicesConfig::from_json("{}").unwrap();
        assert!(!cfg.has_any_service());
        assert!(cfg.configured_services().is_empty());
    }

    #[test]
    fn configured_services_lists_linkding() {
        let cfg = SaveServicesConfig::from_json(
            r#"{"linkding":{"api_url":"https://l","api_token":"t"}}"#,
        )
        .unwrap();
        assert_eq!(cfg.configured_services(), vec!["linkding"]);
    }
}
