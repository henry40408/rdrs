//! Small helpers for pulling request metadata out of HTTP headers.

use axum::http::HeaderMap;

/// Maximum length (in characters) of a captured `User-Agent`, to bound
/// storage — clients can send arbitrarily long values.
const USER_AGENT_MAX_CHARS: usize = 512;

/// The request's `User-Agent` header, truncated to
/// [`USER_AGENT_MAX_CHARS`] characters. Empty string if absent or not valid
/// UTF-8.
pub fn request_user_agent(headers: &HeaderMap) -> String {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(USER_AGENT_MAX_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_empty_string_when_missing() {
        let headers = HeaderMap::new();
        assert_eq!(request_user_agent(&headers), "");
    }

    #[test]
    fn returns_user_agent_when_present() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::USER_AGENT,
            "Mozilla/5.0".parse().unwrap(),
        );
        assert_eq!(request_user_agent(&headers), "Mozilla/5.0");
    }

    #[test]
    fn truncates_to_max_chars() {
        let mut headers = HeaderMap::new();
        let long = "a".repeat(1000);
        headers.insert(axum::http::header::USER_AGENT, long.parse().unwrap());
        let result = request_user_agent(&headers);
        assert_eq!(result.chars().count(), USER_AGENT_MAX_CHARS);
    }
}
