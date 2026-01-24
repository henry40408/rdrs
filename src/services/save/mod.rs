pub mod linkding;

use serde::{Deserialize, Serialize};

pub use linkding::LinkdingConfig;

/// Bookmark data to save to external services
#[derive(Debug, Clone)]
pub struct BookmarkData {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

/// Result of saving to a single service
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
    // Future services can be added here:
    // pub pocket: Option<PocketConfig>,
    // pub wallabag: Option<WallabagConfig>,
}

impl SaveServicesConfig {
    /// Parse JSON string into SaveServicesConfig
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Serialize to JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Get list of configured service names
    pub fn configured_services(&self) -> Vec<&'static str> {
        let mut services = Vec::new();
        if self
            .linkding
            .as_ref()
            .map(|c| c.is_configured())
            .unwrap_or(false)
        {
            services.push("linkding");
        }
        // Add more services here as they are implemented
        services
    }

    /// Check if any service is configured
    pub fn has_any_service(&self) -> bool {
        !self.configured_services().is_empty()
    }
}
