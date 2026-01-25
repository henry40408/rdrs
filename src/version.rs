/// Git 版本 (e.g., "v0.1.0", "v0.1.0-3-g1234567", "1234567")
pub const GIT_VERSION: &str = env!("GIT_VERSION");

/// Cargo.toml 中的版本
pub const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
