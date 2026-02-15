use axum::Json;
use serde::Serialize;

use crate::version::GIT_VERSION;

#[derive(Serialize)]
pub struct HealthResponse {
    status: &'static str,
    git_version: &'static str,
}

pub async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        git_version: GIT_VERSION,
    })
}
