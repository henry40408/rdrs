use axum::{
    extract::FromRequestParts,
    http::request::Parts,
    response::{IntoResponse, IntoResponseParts, Response, ResponseParts},
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use chrono::{DateTime, Utc};
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
    #[serde(default = "Utc::now")]
    pub timestamp: DateTime<Utc>,
}

impl FlashMessage {
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            level: FlashLevel::Success,
            message: message.into(),
            timestamp: Utc::now(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            level: FlashLevel::Error,
            message: message.into(),
            timestamp: Utc::now(),
        }
    }

    pub fn info(message: impl Into<String>) -> Self {
        Self {
            level: FlashLevel::Info,
            message: message.into(),
            timestamp: Utc::now(),
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            level: FlashLevel::Warning,
            message: message.into(),
            timestamp: Utc::now(),
        }
    }

    pub fn level_class(&self) -> &'static str {
        match self.level {
            FlashLevel::Success => "banner--success",
            FlashLevel::Error => "banner--error",
            FlashLevel::Info => "banner--info",
            FlashLevel::Warning => "banner--warning",
        }
    }

    /// ARIA role for live-region announcement.
    /// `status` (polite) for non-blocking info; `alert` (assertive) for problems.
    pub fn aria_role(&self) -> &'static str {
        match self.level {
            FlashLevel::Success | FlashLevel::Info => "status",
            FlashLevel::Warning | FlashLevel::Error => "alert",
        }
    }

    /// Human-readable level name announced by screen readers (visually hidden).
    pub fn level_name(&self) -> &'static str {
        match self.level {
            FlashLevel::Success => "Success",
            FlashLevel::Error => "Error",
            FlashLevel::Info => "Info",
            FlashLevel::Warning => "Warning",
        }
    }

    /// UTC HH:MM:SS — the no-JS fallback only. The server cannot know the
    /// viewer's timezone, so `rdrs-flash.js` rewrites this text from
    /// `timestamp_iso()` into local time on load, matching the banners it
    /// builds itself. Do not add a timezone-dependent format here.
    pub fn formatted_time(&self) -> String {
        self.timestamp.format("%H:%M:%S").to_string()
    }

    /// ISO-8601 form used for the `<time datetime="…">` attribute so
    /// assistive tech, `Date.parse()` consumers and the client-side
    /// localization above get an unambiguous instant.
    pub fn timestamp_iso(&self) -> String {
        self.timestamp.to_rfc3339()
    }
}

const MAX_FLASH_MESSAGES: usize = 3;

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

        let mut messages = jar
            .get(FLASH_COOKIE_NAME)
            .and_then(|cookie| serde_json::from_str::<Vec<FlashMessage>>(cookie.value()).ok())
            .unwrap_or_default();

        // Keep only the latest MAX_FLASH_MESSAGES
        if messages.len() > MAX_FLASH_MESSAGES {
            messages = messages.split_off(messages.len() - MAX_FLASH_MESSAGES);
        }

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
            // Clear the flash cookie after reading. The removal cookie must carry
            // the same `Path=/` the cookie was stored with, otherwise (per RFC 6265)
            // the browser computes a default-path from the request URI and the
            // deletion fails to match on multi-segment routes (see #247).
            let removal = Cookie::build((FLASH_COOKIE_NAME, "")).path("/").build();
            let jar = jar.remove(removal);
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
        (self.flash, axum::response::Redirect::to(&self.location)).into_response()
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
        assert_eq!(FlashMessage::success("").level_class(), "banner--success");
        assert_eq!(FlashMessage::error("").level_class(), "banner--error");
        assert_eq!(FlashMessage::info("").level_class(), "banner--info");
        assert_eq!(FlashMessage::warning("").level_class(), "banner--warning");
    }

    #[test]
    fn test_flash_aria_role() {
        assert_eq!(FlashMessage::success("").aria_role(), "status");
        assert_eq!(FlashMessage::info("").aria_role(), "status");
        assert_eq!(FlashMessage::warning("").aria_role(), "alert");
        assert_eq!(FlashMessage::error("").aria_role(), "alert");
    }

    #[test]
    fn test_flash_level_name() {
        assert_eq!(FlashMessage::success("").level_name(), "Success");
        assert_eq!(FlashMessage::error("").level_name(), "Error");
        assert_eq!(FlashMessage::info("").level_name(), "Info");
        assert_eq!(FlashMessage::warning("").level_name(), "Warning");
    }

    #[test]
    fn test_flash_message_constructors() {
        let success = FlashMessage::success("Success!");
        assert_eq!(success.level, FlashLevel::Success);
        assert_eq!(success.message, "Success!");

        let error = FlashMessage::error("Error!");
        assert_eq!(error.level, FlashLevel::Error);
        assert_eq!(error.message, "Error!");

        let info = FlashMessage::info("Info!");
        assert_eq!(info.level, FlashLevel::Info);
        assert_eq!(info.message, "Info!");

        let warning = FlashMessage::warning("Warning!");
        assert_eq!(warning.level, FlashLevel::Warning);
        assert_eq!(warning.message, "Warning!");
    }

    #[test]
    fn test_flash_message_formatted_time() {
        let msg = FlashMessage::success("Test");
        let time = msg.formatted_time();
        // Format should be HH:MM:SS
        assert_eq!(time.len(), 8);
        assert!(time.contains(':'));
    }

    #[test]
    fn test_flash_message_timestamp_iso() {
        let msg = FlashMessage::success("Test");
        let iso = msg.timestamp_iso();
        // RFC 3339 form: "YYYY-MM-DDTHH:MM:SS(.fff)+ZZ:ZZ" — at minimum
        // contains a date marker and a timezone offset.
        assert!(iso.starts_with(&msg.timestamp.format("%Y-%m-%d").to_string()));
        assert!(iso.contains('T'));
        assert!(iso.contains('+') || iso.contains('Z') || iso.contains('-'));
    }

    #[test]
    fn test_flash_is_empty() {
        let empty_flash = Flash {
            messages: vec![],
            jar: None,
        };
        assert!(empty_flash.is_empty());

        let flash_with_messages = Flash {
            messages: vec![FlashMessage::success("Test")],
            jar: None,
        };
        assert!(!flash_with_messages.is_empty());
    }

    #[test]
    fn test_set_flash_constructors() {
        let success = SetFlash::success("Success!");
        assert_eq!(success.messages.len(), 1);
        assert_eq!(success.messages[0].level, FlashLevel::Success);

        let error = SetFlash::error("Error!");
        assert_eq!(error.messages.len(), 1);
        assert_eq!(error.messages[0].level, FlashLevel::Error);

        let info = SetFlash::info("Info!");
        assert_eq!(info.messages.len(), 1);
        assert_eq!(info.messages[0].level, FlashLevel::Info);

        let warning = SetFlash::warning("Warning!");
        assert_eq!(warning.messages.len(), 1);
        assert_eq!(warning.messages[0].level, FlashLevel::Warning);
    }

    #[test]
    fn test_set_flash_new() {
        let msg = FlashMessage::success("Test message");
        let set_flash = SetFlash::new(msg);
        assert_eq!(set_flash.messages.len(), 1);
        assert_eq!(set_flash.messages[0].message, "Test message");
    }

    #[test]
    fn test_set_flash_messages() {
        let messages = vec![
            FlashMessage::success("First"),
            FlashMessage::error("Second"),
            FlashMessage::info("Third"),
        ];
        let set_flash = SetFlash::messages(messages);
        assert_eq!(set_flash.messages.len(), 3);
        assert_eq!(set_flash.messages[0].level, FlashLevel::Success);
        assert_eq!(set_flash.messages[1].level, FlashLevel::Error);
        assert_eq!(set_flash.messages[2].level, FlashLevel::Info);
    }

    #[test]
    fn test_flash_redirect_constructors() {
        let success = FlashRedirect::success("/", "Success!");
        assert_eq!(success.location, "/");
        assert_eq!(success.flash.messages[0].level, FlashLevel::Success);

        let error = FlashRedirect::error("/login", "Error!");
        assert_eq!(error.location, "/login");
        assert_eq!(error.flash.messages[0].level, FlashLevel::Error);

        let info = FlashRedirect::info("/dashboard", "Info!");
        assert_eq!(info.location, "/dashboard");
        assert_eq!(info.flash.messages[0].level, FlashLevel::Info);

        let warning = FlashRedirect::warning("/settings", "Warning!");
        assert_eq!(warning.location, "/settings");
        assert_eq!(warning.flash.messages[0].level, FlashLevel::Warning);
    }

    #[test]
    fn test_flash_redirect_to() {
        let msg = FlashMessage::success("Custom message");
        let redirect = FlashRedirect::to("/custom", msg);
        assert_eq!(redirect.location, "/custom");
        assert_eq!(redirect.flash.messages[0].message, "Custom message");
    }

    #[test]
    fn test_flash_level_serialization() {
        let success_json = serde_json::to_string(&FlashLevel::Success).unwrap();
        assert_eq!(success_json, "\"success\"");

        let error_json = serde_json::to_string(&FlashLevel::Error).unwrap();
        assert_eq!(error_json, "\"error\"");

        let info_json = serde_json::to_string(&FlashLevel::Info).unwrap();
        assert_eq!(info_json, "\"info\"");

        let warning_json = serde_json::to_string(&FlashLevel::Warning).unwrap();
        assert_eq!(warning_json, "\"warning\"");
    }

    #[test]
    fn test_flash_level_deserialization() {
        let success: FlashLevel = serde_json::from_str("\"success\"").unwrap();
        assert_eq!(success, FlashLevel::Success);

        let error: FlashLevel = serde_json::from_str("\"error\"").unwrap();
        assert_eq!(error, FlashLevel::Error);

        let info: FlashLevel = serde_json::from_str("\"info\"").unwrap();
        assert_eq!(info, FlashLevel::Info);

        let warning: FlashLevel = serde_json::from_str("\"warning\"").unwrap();
        assert_eq!(warning, FlashLevel::Warning);
    }

    #[test]
    fn test_flash_message_json_roundtrip() {
        let original = FlashMessage::error("Test error message");
        let json = serde_json::to_string(&original).unwrap();
        let parsed: FlashMessage = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.level, original.level);
        assert_eq!(parsed.message, original.message);
        // Note: timestamp comparison may differ due to serialization
    }

    #[test]
    fn test_multiple_flash_messages_serialization() {
        let messages = vec![
            FlashMessage::success("First"),
            FlashMessage::error("Second"),
        ];
        let json = serde_json::to_string(&messages).unwrap();
        let parsed: Vec<FlashMessage> = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].message, "First");
        assert_eq!(parsed[1].message, "Second");
    }

    #[test]
    fn test_flash_cookie_name() {
        assert_eq!(FLASH_COOKIE_NAME, "flash");
    }

    #[test]
    fn test_flash_message_with_string_ownership() {
        let msg_string = String::from("Owned string");
        let msg = FlashMessage::success(msg_string);
        assert_eq!(msg.message, "Owned string");

        let msg_str = "String slice";
        let msg = FlashMessage::error(msg_str);
        assert_eq!(msg.message, "String slice");
    }

    #[test]
    fn test_flash_default() {
        let flash = Flash::default();
        assert!(flash.is_empty());
        assert!(flash.jar.is_none());
    }

    #[test]
    fn test_flash_clone() {
        let original = FlashMessage::success("Clone me");
        let cloned = original.clone();
        assert_eq!(cloned.level, original.level);
        assert_eq!(cloned.message, original.message);
    }

    #[test]
    fn test_set_flash_clone() {
        let original = SetFlash::success("Clone me");
        let cloned = original.clone();
        assert_eq!(cloned.messages.len(), original.messages.len());
        assert_eq!(cloned.messages[0].message, original.messages[0].message);
    }

    #[test]
    fn test_flash_redirect_location_ownership() {
        let location = String::from("/dynamic/path");
        let redirect = FlashRedirect::success(location, "Message");
        assert_eq!(redirect.location, "/dynamic/path");
    }

    /// Regression for #247: the flash-cookie removal must carry `Path=/` so it
    /// matches the stored cookie (always written with `Path=/`). Without it the
    /// browser derives a default-path from the request URI, and on multi-segment
    /// routes (e.g. `/feeds/{id}/entries`) the deletion never matches — leaving
    /// the flash to re-render on every subsequent page load.
    #[tokio::test]
    async fn test_flash_removal_cookie_has_root_path() {
        use axum::http::Request;
        use axum::response::IntoResponse;

        let messages = vec![FlashMessage::success("Marked 5 loaded entries as read.")];
        let cookie_value = serde_json::to_string(&messages).unwrap();

        // Reproduce the original report: consume the flash on a multi-segment route.
        let request = Request::builder()
            .uri("/feeds/5/entries")
            .header("Cookie", format!("{FLASH_COOKIE_NAME}={cookie_value}"))
            .body(())
            .unwrap();
        let (mut parts, ()) = request.into_parts();

        let flash = Flash::from_request_parts(&mut parts, &()).await.unwrap();
        assert_eq!(flash.messages.len(), 1, "flash cookie should be read");

        let response = (flash, ()).into_response();
        let removal = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap().to_string())
            .find(|value| value.starts_with(&format!("{FLASH_COOKIE_NAME}=")))
            .expect("removal Set-Cookie header for flash should be present");

        assert!(
            removal.contains("Path=/"),
            "removal cookie must carry Path=/ to match the stored cookie, got: {removal}"
        );
    }
}
