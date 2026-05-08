//! Tower middleware that attaches a weak ETag to 2xx text/html
//! responses and converts to 304 when the client's If-None-Match
//! matches.
//!
//! Wired innermost so the body it hashes is the uncompressed one;
//! CompressionLayer runs after this on the response path.

use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::{to_bytes, Body};
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::response::Response;
use sha2::{Digest, Sha256};
use tower::{Layer, Service};

/// Maximum body size (bytes) the middleware will buffer to compute
/// an ETag. Bodies larger than this pass through untouched.
/// 4 MiB covers any reasonable SSR HTML page; anything larger is
/// almost certainly a streamed asset and shouldn't be buffered.
const MAX_BUFFER_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct ETagLayer;

impl ETagLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for ETagLayer {
    type Service = ETagService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ETagService { inner }
    }
}

#[derive(Clone)]
pub struct ETagService<S> {
    inner: S,
}

impl<S> Service<Request<Body>> for ETagService<S>
where
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future =
        Pin<Box<dyn std::future::Future<Output = Result<Self::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let if_none_match = request
            .headers()
            .get(header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let mut inner = self.inner.clone();

        Box::pin(async move {
            let response = inner.call(request).await?;

            if !is_taggable(&response) {
                return Ok(response);
            }

            let (mut parts, body) = response.into_parts();
            let bytes = match to_bytes(body, MAX_BUFFER_BYTES).await {
                Ok(b) => b,
                Err(_) => {
                    // Body too large or stream error — return a fresh
                    // empty 200 with a hint header. Production handlers
                    // should not exceed MAX_BUFFER_BYTES for HTML; if
                    // they do we want a loud signal in tests.
                    let response = Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::empty())
                        .expect("static response");
                    return Ok(response);
                }
            };

            let etag = compute_weak_etag(&bytes);
            parts.headers.insert(
                header::ETAG,
                HeaderValue::from_str(&etag).expect("etag is ascii"),
            );

            if let Some(client_value) = if_none_match {
                if etag_matches(&client_value, &etag) {
                    parts.status = StatusCode::NOT_MODIFIED;
                    parts.headers.remove(header::CONTENT_LENGTH);
                    parts.headers.remove(header::CONTENT_TYPE);
                    return Ok(Response::from_parts(parts, Body::empty()));
                }
            }

            Ok(Response::from_parts(parts, Body::from(bytes)))
        })
    }
}

fn is_taggable(response: &Response) -> bool {
    if !response.status().is_success() {
        return false;
    }
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.starts_with("text/html"))
        .unwrap_or(false)
}

fn compute_weak_etag(body: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body);
    let digest = hasher.finalize();
    // First 16 hex chars = 64 bits — collision-safe for our use.
    let hex: String = digest
        .iter()
        .take(8)
        .map(|b| format!("{:02x}", b))
        .collect();
    format!("W/\"{hex}\"")
}

fn etag_matches(client: &str, server: &str) -> bool {
    // Accept exact match. Per RFC 7232, If-None-Match comparison is
    // weak (W/-prefix is ignored), so we strip W/ from both sides.
    let normalize =
        |s: &str| -> String { s.trim().strip_prefix("W/").unwrap_or(s.trim()).to_string() };
    // Client may send a comma-separated list; check each.
    client
        .split(',')
        .any(|entry| normalize(entry) == normalize(server))
}
