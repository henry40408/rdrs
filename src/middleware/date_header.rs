use axum::{body::Body, http::Request, response::Response};
use std::task::{Context, Poll};
use tower::{Layer, Service};

/// Middleware layer that adds HTTP Date header to all responses
#[derive(Clone)]
pub struct DateHeaderLayer;

impl DateHeaderLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for DateHeaderLayer {
    type Service = DateHeaderService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DateHeaderService { inner }
    }
}

#[derive(Clone)]
pub struct DateHeaderService<S> {
    inner: S,
}

impl<S> Service<Request<Body>> for DateHeaderService<S>
where
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let mut inner = self.inner.clone();

        Box::pin(async move {
            let mut response = inner.call(request).await?;

            // Add Date header in RFC 7231 format (e.g., "Mon, 23 Feb 2026 01:23:45 GMT")
            let now = chrono::Utc::now();
            let date_str = now.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
            response
                .headers_mut()
                .insert("Date", date_str.parse().unwrap());

            Ok(response)
        })
    }
}
