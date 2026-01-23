use axum::{
    extract::FromRequestParts,
    http::request::Parts,
    response::{IntoResponse, IntoResponseParts, Response, ResponseParts},
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde::{Deserialize, Serialize};

pub const FLASH_COOKIE_NAME: &str = "flash";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FlashLevel {
    Success,
    Error,
    Info,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlashMessage {
    pub level: FlashLevel,
    pub message: String,
}

impl FlashMessage {
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            level: FlashLevel::Success,
            message: message.into(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            level: FlashLevel::Error,
            message: message.into(),
        }
    }

    pub fn info(message: impl Into<String>) -> Self {
        Self {
            level: FlashLevel::Info,
            message: message.into(),
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            level: FlashLevel::Warning,
            message: message.into(),
        }
    }

    pub fn level_class(&self) -> &'static str {
        match self.level {
            FlashLevel::Success => "flash-success",
            FlashLevel::Error => "flash-error",
            FlashLevel::Info => "flash-info",
            FlashLevel::Warning => "flash-warning",
        }
    }
}

/// Extractor that reads and clears flash messages from cookies
#[derive(Debug, Clone, Default)]
pub struct Flash {
    pub messages: Vec<FlashMessage>,
    jar: Option<CookieJar>,
}

impl Flash {
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

impl<S> FromRequestParts<S> for Flash
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_request_parts(parts, state)
            .await
            .unwrap_or_default();

        let messages = jar
            .get(FLASH_COOKIE_NAME)
            .and_then(|cookie| serde_json::from_str::<Vec<FlashMessage>>(cookie.value()).ok())
            .unwrap_or_default();

        Ok(Flash {
            messages,
            jar: Some(jar),
        })
    }
}

impl IntoResponseParts for Flash {
    type Error = std::convert::Infallible;

    fn into_response_parts(self, res: ResponseParts) -> Result<ResponseParts, Self::Error> {
        if let Some(jar) = self.jar {
            // Clear the flash cookie after reading
            let jar = jar.remove(FLASH_COOKIE_NAME);
            jar.into_response_parts(res)
        } else {
            Ok(res)
        }
    }
}

/// Response type that sets flash messages
#[derive(Debug, Clone)]
pub struct SetFlash {
    messages: Vec<FlashMessage>,
}

impl SetFlash {
    pub fn new(message: FlashMessage) -> Self {
        Self {
            messages: vec![message],
        }
    }

    pub fn messages(messages: Vec<FlashMessage>) -> Self {
        Self { messages }
    }

    pub fn success(message: impl Into<String>) -> Self {
        Self::new(FlashMessage::success(message))
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(FlashMessage::error(message))
    }

    pub fn info(message: impl Into<String>) -> Self {
        Self::new(FlashMessage::info(message))
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(FlashMessage::warning(message))
    }
}

impl IntoResponseParts for SetFlash {
    type Error = std::convert::Infallible;

    fn into_response_parts(self, res: ResponseParts) -> Result<ResponseParts, Self::Error> {
        let json = serde_json::to_string(&self.messages).unwrap_or_default();
        let cookie = Cookie::build((FLASH_COOKIE_NAME, json))
            .path("/")
            .http_only(true)
            .same_site(axum_extra::extract::cookie::SameSite::Lax)
            .build();

        let jar = CookieJar::new().add(cookie);
        jar.into_response_parts(res)
    }
}

impl IntoResponse for SetFlash {
    fn into_response(self) -> Response {
        (self, ()).into_response()
    }
}

/// Helper to create a redirect response with a flash message
pub struct FlashRedirect {
    pub flash: SetFlash,
    pub location: String,
}

impl FlashRedirect {
    pub fn to(location: impl Into<String>, message: FlashMessage) -> Self {
        Self {
            flash: SetFlash::new(message),
            location: location.into(),
        }
    }

    pub fn success(location: impl Into<String>, message: impl Into<String>) -> Self {
        Self::to(location, FlashMessage::success(message))
    }

    pub fn error(location: impl Into<String>, message: impl Into<String>) -> Self {
        Self::to(location, FlashMessage::error(message))
    }

    pub fn info(location: impl Into<String>, message: impl Into<String>) -> Self {
        Self::to(location, FlashMessage::info(message))
    }

    pub fn warning(location: impl Into<String>, message: impl Into<String>) -> Self {
        Self::to(location, FlashMessage::warning(message))
    }
}

impl IntoResponse for FlashRedirect {
    fn into_response(self) -> Response {
        (
            self.flash,
            axum::response::Redirect::to(&self.location),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flash_message_serialization() {
        let msg = FlashMessage::success("Test message");
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: FlashMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.level, FlashLevel::Success);
        assert_eq!(parsed.message, "Test message");
    }

    #[test]
    fn test_flash_level_class() {
        assert_eq!(FlashMessage::success("").level_class(), "flash-success");
        assert_eq!(FlashMessage::error("").level_class(), "flash-error");
        assert_eq!(FlashMessage::info("").level_class(), "flash-info");
        assert_eq!(FlashMessage::warning("").level_class(), "flash-warning");
    }
}
