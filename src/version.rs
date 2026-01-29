/// Git version (e.g., "v0.1.0", "v0.1.0-3-g1234567", "1234567")
pub const GIT_VERSION: &str = env!("GIT_VERSION");

/// Version from Cargo.toml
pub const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
