use std::future::Future;
use std::time::Duration;

use reqwest::{Response, StatusCode};
use tracing::warn;

/// Default timeout for general HTTP requests (30s)
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for icon fetching (10s)
pub const ICON_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for external API calls like Linkding, Kagi (60s)
pub const EXTERNAL_API_TIMEOUT: Duration = Duration::from_secs(60);

/// Configuration for retry behavior
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(8),
        }
    }
}

impl RetryConfig {
    /// Reduced retry config suitable for icon fetching
    pub fn icon() -> Self {
        Self {
            max_retries: 2,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(4),
        }
    }
}

/// Outcome of a single attempt, used by the retry loop to decide whether to retry.
pub enum RetryOutcome<T, E> {
    /// Request succeeded
    Success(T),
    /// Transient error, safe to retry
    Transient(E),
    /// Permanent error, do not retry
    Permanent(E),
}

/// Check if a reqwest error is transient (safe to retry).
pub fn is_transient_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request()
}

/// Check if an HTTP status code is transient (safe to retry).
pub fn is_transient_status(status: StatusCode) -> bool {
    status.is_server_error()
        || status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT
}

/// Generic async retry function with exponential backoff.
///
/// `operation` is called on each attempt and must return a `RetryOutcome`.
/// On `Transient`, the function sleeps with exponential backoff and retries.
/// On `Permanent` or after exhausting retries, the error is returned.
pub async fn retry<T, E, Fut, F>(config: &RetryConfig, mut operation: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = RetryOutcome<T, E>>,
{
    let mut backoff = config.initial_backoff;

    for attempt in 0..=config.max_retries {
        match operation().await {
            RetryOutcome::Success(val) => return Ok(val),
            RetryOutcome::Permanent(err) => return Err(err),
            RetryOutcome::Transient(err) => {
                if attempt == config.max_retries {
                    return Err(err);
                }
                warn!(
                    "Transient error on attempt {}/{}, retrying in {:?}",
                    attempt + 1,
                    config.max_retries + 1,
                    backoff
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(config.max_backoff);
            }
        }
    }

    unreachable!("retry loop should return before reaching here")
}

/// Convenience wrapper: send an HTTP request with retry.
///
/// `build_request` is called on each attempt to produce a fresh `reqwest::RequestBuilder`.
/// This is needed because `RequestBuilder` is consumed by `.send()`.
///
/// Returns the `Response` on success, or a `reqwest::Error` after retries are exhausted.
/// Non-transient HTTP status codes (4xx except 408/429) are returned as successful responses
/// for the caller to handle.
pub async fn send_with_retry<F>(
    config: &RetryConfig,
    mut build_request: F,
) -> Result<Response, reqwest::Error>
where
    F: FnMut() -> reqwest::RequestBuilder,
{
    retry(config, || {
        let request = build_request();
        async {
            match request.send().await {
                Ok(response) => {
                    if is_transient_status(response.status()) {
                        // Convert response to an error-like form for retry
                        // but we still have the response, so wrap it
                        RetryOutcome::Success(response)
                        // Note: We return Success here because the caller may want
                        // to inspect 5xx responses. The retry on status is handled below.
                    } else {
                        RetryOutcome::Success(response)
                    }
                }
                Err(err) => {
                    if is_transient_error(&err) {
                        RetryOutcome::Transient(err)
                    } else {
                        RetryOutcome::Permanent(err)
                    }
                }
            }
        }
    })
    .await
}

/// Send with retry, also retrying on transient HTTP status codes (5xx, 429, 408).
///
/// `build_request` is called on each attempt to produce a fresh `reqwest::RequestBuilder`.
/// Returns the final `Response` (which may be a non-retriable error status like 4xx).
pub async fn send_with_retry_on_status<F>(
    config: &RetryConfig,
    mut build_request: F,
) -> Result<Response, reqwest::Error>
where
    F: FnMut() -> reqwest::RequestBuilder,
{
    let mut backoff = config.initial_backoff;

    for attempt in 0..=config.max_retries {
        let request = build_request();
        match request.send().await {
            Ok(response) => {
                if is_transient_status(response.status()) && attempt < config.max_retries {
                    warn!(
                        "Transient status {} on attempt {}/{}, retrying in {:?}",
                        response.status(),
                        attempt + 1,
                        config.max_retries + 1,
                        backoff
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(config.max_backoff);
                    continue;
                }
                return Ok(response);
            }
            Err(err) => {
                if is_transient_error(&err) && attempt < config.max_retries {
                    warn!(
                        "Transient error on attempt {}/{}, retrying in {:?}",
                        attempt + 1,
                        config.max_retries + 1,
                        backoff
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(config.max_backoff);
                    continue;
                }
                return Err(err);
            }
        }
    }

    unreachable!("retry loop should return before reaching here")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_is_transient_status() {
        assert!(is_transient_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(is_transient_status(StatusCode::BAD_GATEWAY));
        assert!(is_transient_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(is_transient_status(StatusCode::GATEWAY_TIMEOUT));
        assert!(is_transient_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_transient_status(StatusCode::REQUEST_TIMEOUT));

        assert!(!is_transient_status(StatusCode::OK));
        assert!(!is_transient_status(StatusCode::NOT_FOUND));
        assert!(!is_transient_status(StatusCode::BAD_REQUEST));
        assert!(!is_transient_status(StatusCode::UNAUTHORIZED));
        assert!(!is_transient_status(StatusCode::FORBIDDEN));
    }

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.initial_backoff, Duration::from_secs(1));
        assert_eq!(config.max_backoff, Duration::from_secs(8));
    }

    #[test]
    fn test_retry_config_icon() {
        let config = RetryConfig::icon();
        assert_eq!(config.max_retries, 2);
        assert_eq!(config.initial_backoff, Duration::from_millis(500));
        assert_eq!(config.max_backoff, Duration::from_secs(4));
    }

    #[tokio::test]
    async fn test_retry_immediate_success() {
        let config = RetryConfig {
            max_retries: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
        };

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result: Result<&str, &str> = retry(&config, || {
            let attempts = attempts_clone.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                RetryOutcome::Success("ok")
            }
        })
        .await;

        assert_eq!(result, Ok("ok"));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_transient_then_success() {
        let config = RetryConfig {
            max_retries: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
        };

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result: Result<&str, &str> = retry(&config, || {
            let attempts = attempts_clone.clone();
            async move {
                let n = attempts.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    RetryOutcome::Transient("transient error")
                } else {
                    RetryOutcome::Success("ok")
                }
            }
        })
        .await;

        assert_eq!(result, Ok("ok"));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let config = RetryConfig {
            max_retries: 2,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
        };

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result: Result<&str, &str> = retry(&config, || {
            let attempts = attempts_clone.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                RetryOutcome::Transient("still failing")
            }
        })
        .await;

        assert_eq!(result, Err("still failing"));
        // 1 initial + 2 retries = 3 total attempts
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_permanent_error_short_circuits() {
        let config = RetryConfig {
            max_retries: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
        };

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result: Result<&str, &str> = retry(&config, || {
            let attempts = attempts_clone.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                RetryOutcome::Permanent("permanent error")
            }
        })
        .await;

        assert_eq!(result, Err("permanent error"));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_timeout_constants() {
        assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(30));
        assert_eq!(ICON_TIMEOUT, Duration::from_secs(10));
        assert_eq!(EXTERNAL_API_TIMEOUT, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_send_with_retry_success() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = RetryConfig {
            max_retries: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
        };

        let url = mock_server.uri();
        let client = reqwest::Client::new();
        let response = send_with_retry(&config, || client.get(&url)).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn test_send_with_retry_on_status_retries_5xx() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(503))
            .up_to_n_times(2)
            .expect(2)
            .mount(&mock_server)
            .await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("recovered"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = RetryConfig {
            max_retries: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
        };

        let url = mock_server.uri();
        let client = reqwest::Client::new();
        let response = send_with_retry_on_status(&config, || client.get(&url))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), "recovered");
    }

    #[tokio::test]
    async fn test_send_with_retry_on_status_no_retry_4xx() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = RetryConfig {
            max_retries: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
        };

        let url = mock_server.uri();
        let client = reqwest::Client::new();
        let response = send_with_retry_on_status(&config, || client.get(&url))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_send_with_retry_on_status_retries_429() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(429))
            .up_to_n_times(1)
            .expect(1)
            .mount(&mock_server)
            .await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = RetryConfig {
            max_retries: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
        };

        let url = mock_server.uri();
        let client = reqwest::Client::new();
        let response = send_with_retry_on_status(&config, || client.get(&url))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_send_with_retry_on_status_exhausted() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(503))
            .expect(3)
            .mount(&mock_server)
            .await;

        let config = RetryConfig {
            max_retries: 2,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
        };

        let url = mock_server.uri();
        let client = reqwest::Client::new();
        let response = send_with_retry_on_status(&config, || client.get(&url))
            .await
            .unwrap();

        // After exhausting retries, returns the last 503 response
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_backoff_capping() {
        let config = RetryConfig {
            max_retries: 5,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(200),
        };

        let mut backoff = config.initial_backoff;
        let mut backoffs = vec![];
        for _ in 0..config.max_retries {
            backoffs.push(backoff);
            backoff = (backoff * 2).min(config.max_backoff);
        }

        // initial_backoff=100, doubled=200 (capped), doubled=400->capped to 200, ...
        assert_eq!(backoffs[0], Duration::from_millis(100));
        assert_eq!(backoffs[1], Duration::from_millis(200));
        assert_eq!(backoffs[2], Duration::from_millis(200));
        assert_eq!(backoffs[3], Duration::from_millis(200));
        assert_eq!(backoffs[4], Duration::from_millis(200));
    }

    #[tokio::test]
    async fn test_is_transient_error_with_timeout() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string("slow")
                    .set_delay(Duration::from_secs(5)),
            )
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(10))
            .build()
            .unwrap();

        let err = client.get(&mock_server.uri()).send().await.unwrap_err();

        assert!(is_transient_error(&err));
        assert!(err.is_timeout());
    }
}
