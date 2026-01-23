use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("User not found")]
    UserNotFound,

    #[error("Username already exists")]
    UsernameExists,

    #[error("Registration not allowed")]
    RegistrationNotAllowed,

    #[error("User is disabled")]
    UserDisabled,

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Forbidden")]
    Forbidden,

    #[error("Cannot modify self")]
    CannotModifySelf,

    #[error("Already masquerading")]
    AlreadyMasquerading,

    #[error("Not masquerading")]
    NotMasquerading,

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Internal server error")]
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Database error"),
            AppError::InvalidCredentials => (StatusCode::UNAUTHORIZED, "Invalid credentials"),
            AppError::UserNotFound => (StatusCode::NOT_FOUND, "User not found"),
            AppError::UsernameExists => (StatusCode::CONFLICT, "Username already exists"),
            AppError::RegistrationNotAllowed => {
                (StatusCode::FORBIDDEN, "Registration not allowed")
            }
            AppError::UserDisabled => (StatusCode::FORBIDDEN, "User is disabled"),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized"),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "Forbidden"),
            AppError::CannotModifySelf => (StatusCode::BAD_REQUEST, "Cannot modify self"),
            AppError::AlreadyMasquerading => (StatusCode::BAD_REQUEST, "Already masquerading"),
            AppError::NotMasquerading => (StatusCode::BAD_REQUEST, "Not masquerading"),
            AppError::Validation(msg) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response(),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
        };

        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
