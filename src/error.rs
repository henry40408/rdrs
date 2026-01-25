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

    #[error("Passkey not found")]
    PasskeyNotFound,

    #[error("Passkey registration failed: {0}")]
    PasskeyRegistrationFailed(String),

    #[error("Passkey authentication failed: {0}")]
    PasskeyAuthenticationFailed(String),

    #[error("Challenge not found or expired")]
    ChallengeNotFound,

    #[error("{0}")]
    NotFound(String),

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
            AppError::PasskeyNotFound => (StatusCode::NOT_FOUND, "Passkey not found"),
            AppError::PasskeyRegistrationFailed(msg) => {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
            }
            AppError::PasskeyAuthenticationFailed(msg) => {
                return (StatusCode::UNAUTHORIZED, Json(json!({ "error": msg }))).into_response()
            }
            AppError::ChallengeNotFound => {
                (StatusCode::BAD_REQUEST, "Challenge not found or expired")
            }
            AppError::NotFound(msg) => {
                return (StatusCode::NOT_FOUND, Json(json!({ "error": msg }))).into_response()
            }
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
        };

        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    async fn get_response_body(response: Response) -> String {
        let body = response.into_body();
        let bytes = body.collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn test_database_error_response() {
        let err = AppError::Database(rusqlite::Error::QueryReturnedNoRows);
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = get_response_body(response).await;
        assert!(body.contains("Database error"));
    }

    #[tokio::test]
    async fn test_invalid_credentials_response() {
        let err = AppError::InvalidCredentials;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = get_response_body(response).await;
        assert!(body.contains("Invalid credentials"));
    }

    #[tokio::test]
    async fn test_user_not_found_response() {
        let err = AppError::UserNotFound;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = get_response_body(response).await;
        assert!(body.contains("User not found"));
    }

    #[tokio::test]
    async fn test_username_exists_response() {
        let err = AppError::UsernameExists;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = get_response_body(response).await;
        assert!(body.contains("Username already exists"));
    }

    #[tokio::test]
    async fn test_registration_not_allowed_response() {
        let err = AppError::RegistrationNotAllowed;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = get_response_body(response).await;
        assert!(body.contains("Registration not allowed"));
    }

    #[tokio::test]
    async fn test_user_disabled_response() {
        let err = AppError::UserDisabled;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = get_response_body(response).await;
        assert!(body.contains("User is disabled"));
    }

    #[tokio::test]
    async fn test_unauthorized_response() {
        let err = AppError::Unauthorized;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = get_response_body(response).await;
        assert!(body.contains("Unauthorized"));
    }

    #[tokio::test]
    async fn test_forbidden_response() {
        let err = AppError::Forbidden;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = get_response_body(response).await;
        assert!(body.contains("Forbidden"));
    }

    #[tokio::test]
    async fn test_cannot_modify_self_response() {
        let err = AppError::CannotModifySelf;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_response_body(response).await;
        assert!(body.contains("Cannot modify self"));
    }

    #[tokio::test]
    async fn test_already_masquerading_response() {
        let err = AppError::AlreadyMasquerading;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_response_body(response).await;
        assert!(body.contains("Already masquerading"));
    }

    #[tokio::test]
    async fn test_not_masquerading_response() {
        let err = AppError::NotMasquerading;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_response_body(response).await;
        assert!(body.contains("Not masquerading"));
    }

    #[tokio::test]
    async fn test_category_not_found_response() {
        let err = AppError::CategoryNotFound;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = get_response_body(response).await;
        assert!(body.contains("Category not found"));
    }

    #[tokio::test]
    async fn test_category_exists_response() {
        let err = AppError::CategoryExists;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = get_response_body(response).await;
        assert!(body.contains("Category already exists"));
    }

    #[tokio::test]
    async fn test_feed_not_found_response() {
        let err = AppError::FeedNotFound;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = get_response_body(response).await;
        assert!(body.contains("Feed not found"));
    }

    #[tokio::test]
    async fn test_feed_exists_response() {
        let err = AppError::FeedExists;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = get_response_body(response).await;
        assert!(body.contains("Feed already exists"));
    }

    #[tokio::test]
    async fn test_entry_not_found_response() {
        let err = AppError::EntryNotFound;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = get_response_body(response).await;
        assert!(body.contains("Entry not found"));
    }

    #[tokio::test]
    async fn test_invalid_url_response() {
        let err = AppError::InvalidUrl;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_response_body(response).await;
        assert!(body.contains("Invalid URL"));
    }

    #[tokio::test]
    async fn test_fetch_error_response() {
        let err = AppError::FetchError("Connection timeout".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = get_response_body(response).await;
        assert!(body.contains("Connection timeout"));
    }

    #[tokio::test]
    async fn test_no_feed_found_response() {
        let err = AppError::NoFeedFound;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_response_body(response).await;
        assert!(body.contains("No feed found at URL"));
    }

    #[tokio::test]
    async fn test_feed_parse_error_response() {
        let err = AppError::FeedParseError("Invalid XML".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_response_body(response).await;
        assert!(body.contains("Invalid XML"));
    }

    #[tokio::test]
    async fn test_validation_error_response() {
        let err = AppError::Validation("Name is required".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_response_body(response).await;
        assert!(body.contains("Name is required"));
    }

    #[tokio::test]
    async fn test_opml_parse_error_response() {
        let err = AppError::OpmlParseError("Invalid OPML structure".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_response_body(response).await;
        assert!(body.contains("Invalid OPML structure"));
    }

    #[tokio::test]
    async fn test_invalid_image_url_response() {
        let err = AppError::InvalidImageUrl;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_response_body(response).await;
        assert!(body.contains("Invalid image URL"));
    }

    #[tokio::test]
    async fn test_image_fetch_error_response() {
        let err = AppError::ImageFetchError("404 Not Found".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = get_response_body(response).await;
        assert!(body.contains("404 Not Found"));
    }

    #[tokio::test]
    async fn test_image_too_large_response() {
        let err = AppError::ImageTooLarge;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_response_body(response).await;
        assert!(body.contains("Image too large"));
    }

    #[tokio::test]
    async fn test_unsupported_image_type_response() {
        let err = AppError::UnsupportedImageType;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_response_body(response).await;
        assert!(body.contains("Unsupported image type"));
    }

    #[tokio::test]
    async fn test_invalid_signature_response() {
        let err = AppError::InvalidSignature;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_response_body(response).await;
        assert!(body.contains("Invalid signature"));
    }

    #[tokio::test]
    async fn test_not_found_response() {
        let err = AppError::NotFound("Resource not found".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = get_response_body(response).await;
        assert!(body.contains("Resource not found"));
    }

    #[tokio::test]
    async fn test_internal_error_response() {
        let err = AppError::Internal("Something went wrong".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = get_response_body(response).await;
        assert!(body.contains("Internal server error"));
    }

    #[tokio::test]
    async fn test_passkey_not_found_response() {
        let err = AppError::PasskeyNotFound;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = get_response_body(response).await;
        assert!(body.contains("Passkey not found"));
    }

    #[tokio::test]
    async fn test_passkey_registration_failed_response() {
        let err = AppError::PasskeyRegistrationFailed("Invalid attestation".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_response_body(response).await;
        assert!(body.contains("Invalid attestation"));
    }

    #[tokio::test]
    async fn test_passkey_authentication_failed_response() {
        let err = AppError::PasskeyAuthenticationFailed("Signature mismatch".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = get_response_body(response).await;
        assert!(body.contains("Signature mismatch"));
    }

    #[tokio::test]
    async fn test_challenge_not_found_response() {
        let err = AppError::ChallengeNotFound;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_response_body(response).await;
        assert!(body.contains("Challenge not found"));
    }

    #[test]
    fn test_error_display() {
        assert_eq!(
            format!("{}", AppError::InvalidCredentials),
            "Invalid credentials"
        );
        assert_eq!(
            format!("{}", AppError::FetchError("timeout".to_string())),
            "Failed to fetch: timeout"
        );
        assert_eq!(
            format!("{}", AppError::Validation("invalid".to_string())),
            "Validation error: invalid"
        );
    }

    #[test]
    fn test_error_from_rusqlite() {
        let sqlite_err = rusqlite::Error::QueryReturnedNoRows;
        let app_err: AppError = sqlite_err.into();
        assert!(matches!(app_err, AppError::Database(_)));
    }
}
