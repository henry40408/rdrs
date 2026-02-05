use axum::Json;
use serde::Serialize;

use crate::version::{GIT_VERSION, PKG_VERSION};

#[derive(Serialize)]
pub struct HealthResponse {
    status: &'static str,
    version: &'static str,
    git_version: &'static str,
}

pub async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: PKG_VERSION,
        git_version: GIT_VERSION,
    })
}
