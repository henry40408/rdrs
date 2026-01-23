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

    #[error("Category not found")]
    CategoryNotFound,

    #[error("Category already exists")]
    CategoryExists,

    #[error("Feed not found")]
    FeedNotFound,

    #[error("Feed already exists")]
    FeedExists,

    #[error("Entry not found")]
    EntryNotFound,

    #[error("Invalid URL")]
    InvalidUrl,

    #[error("Failed to fetch: {0}")]
    FetchError(String),

    #[error("No feed found at URL")]
    NoFeedFound,

    #[error("Failed to parse feed: {0}")]
    FeedParseError(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Invalid OPML format: {0}")]
    OpmlParseError(String),

    #[error("Invalid image URL")]
    InvalidImageUrl,

    #[error("Image fetch failed: {0}")]
    ImageFetchError(String),

    #[error("Image too large")]
    ImageTooLarge,

    #[error("Unsupported image type")]
    UnsupportedImageType,

    #[error("Invalid signature")]
    InvalidSignature,

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
            AppError::RegistrationNotAllowed => (StatusCode::FORBIDDEN, "Registration not allowed"),
            AppError::UserDisabled => (StatusCode::FORBIDDEN, "User is disabled"),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized"),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "Forbidden"),
            AppError::CannotModifySelf => (StatusCode::BAD_REQUEST, "Cannot modify self"),
            AppError::AlreadyMasquerading => (StatusCode::BAD_REQUEST, "Already masquerading"),
            AppError::NotMasquerading => (StatusCode::BAD_REQUEST, "Not masquerading"),
            AppError::CategoryNotFound => (StatusCode::NOT_FOUND, "Category not found"),
            AppError::CategoryExists => (StatusCode::CONFLICT, "Category already exists"),
            AppError::FeedNotFound => (StatusCode::NOT_FOUND, "Feed not found"),
            AppError::FeedExists => (StatusCode::CONFLICT, "Feed already exists"),
            AppError::EntryNotFound => (StatusCode::NOT_FOUND, "Entry not found"),
            AppError::InvalidUrl => (StatusCode::BAD_REQUEST, "Invalid URL"),
            AppError::FetchError(msg) => {
                return (StatusCode::BAD_GATEWAY, Json(json!({ "error": msg }))).into_response()
            }
            AppError::NoFeedFound => (StatusCode::BAD_REQUEST, "No feed found at URL"),
            AppError::FeedParseError(msg) => {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
            }
            AppError::Validation(msg) => {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
            }
            AppError::OpmlParseError(msg) => {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
            }
            AppError::InvalidImageUrl => (StatusCode::BAD_REQUEST, "Invalid image URL"),
            AppError::ImageFetchError(msg) => {
                return (StatusCode::BAD_GATEWAY, Json(json!({ "error": msg }))).into_response()
            }
            AppError::ImageTooLarge => (StatusCode::BAD_REQUEST, "Image too large"),
            AppError::UnsupportedImageType => (StatusCode::BAD_REQUEST, "Unsupported image type"),
            AppError::InvalidSignature => (StatusCode::BAD_REQUEST, "Invalid signature"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
        };

        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
