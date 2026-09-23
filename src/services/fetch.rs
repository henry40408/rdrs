//! The HTTP client for every attacker-influenced fetch.
//!
//! A vetted URL can still redirect inward or resolve (or rebind) to a private
//! address, so the guard lives in the client: every redirect hop is
//! re-validated against the [`FetchPolicy`] (capped at `MAX_REDIRECTS`), and
//! every DNS answer is filtered at connect time, leaving no check/connect
//! window. User-configured endpoints (Linkding, Kagi) deliberately use their
//! own clients.

use std::net::SocketAddr;
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::redirect;
use url::Url;

use crate::services::http::DEFAULT_TIMEOUT;
use crate::utils::url_validation::{FetchPolicy, UrlValidationError, is_private_ip};

/// Redirect cap; shorter than reqwest's 10 to bound time spent on a blocked
/// target.
const MAX_REDIRECTS: usize = 5;

/// Pooled HTTP client whose redirects and DNS answers obey a [`FetchPolicy`].
/// Cheap to clone.
#[derive(Debug, Clone)]
pub struct Fetcher {
    policy: Arc<FetchPolicy>,
    client: reqwest::Client,
    client_h1: reqwest::Client,
}

impl Fetcher {
    /// Build the pooled clients for `policy`. Fails only if TLS cannot initialise.
    pub fn new(policy: FetchPolicy) -> Result<Self, String> {
        let policy = Arc::new(policy);

        let build = |http1_only: bool| -> Result<reqwest::Client, String> {
            let mut builder = reqwest::Client::builder()
                .timeout(DEFAULT_TIMEOUT)
                .redirect(redirect_policy(&policy))
                .dns_resolver(Arc::new(GuardedResolver {
                    policy: policy.clone(),
                }));
            if http1_only {
                builder = builder.http1_only();
            }
            builder
                .build()
                .map_err(|e| format!("failed to build the guarded HTTP client: {e}"))
        };

        Ok(Self {
            client: build(false)?,
            client_h1: build(true)?,
            policy,
        })
    }

    /// The client for a feed, honouring its HTTP/2 opt-out (`http1_only()` is
    /// client-level, hence two pools).
    pub fn client(&self, http2_disabled: bool) -> &reqwest::Client {
        if http2_disabled {
            &self.client_h1
        } else {
            &self.client
        }
    }

    /// Early URL check for a reportable error; the client still re-checks every
    /// hop and resolved address.
    pub fn validate(&self, url: &Url) -> Result<(), UrlValidationError> {
        self.policy.validate(url)
    }
}

impl Default for Fetcher {
    /// A fetcher that allows nothing beyond the public internet.
    fn default() -> Self {
        Self::new(FetchPolicy::default()).expect("the default HTTP client must build")
    }
}

fn redirect_policy(policy: &Arc<FetchPolicy>) -> redirect::Policy {
    let policy = policy.clone();
    redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            return attempt.error(RedirectRefused::TooMany);
        }
        let target = attempt.url().clone();
        match policy.validate(&target) {
            Ok(()) => attempt.follow(),
            Err(_) => attempt.error(RedirectRefused::Blocked(target)),
        }
    })
}

/// Why a redirect was not followed; surfaces through `reqwest::Error`.
#[derive(Debug)]
enum RedirectRefused {
    Blocked(Url),
    TooMany,
}

impl std::fmt::Display for RedirectRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RedirectRefused::Blocked(url) => {
                write!(f, "refused to follow a redirect to {url}")
            }
            RedirectRefused::TooMany => write!(f, "too many redirects (limit {MAX_REDIRECTS})"),
        }
    }
}

impl std::error::Error for RedirectRefused {}

/// The system resolver, with every answer the policy would refuse dropped.
#[derive(Debug)]
struct GuardedResolver {
    policy: Arc<FetchPolicy>,
}

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = self.policy.clone();
        Box::pin(async move {
            let host = name.as_str().to_owned();
            // An allow-listed host is trusted wherever it points.
            let host_allowed = policy.allows(&host);

            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
                .collect();

            let kept: Vec<SocketAddr> = addrs
                .into_iter()
                .filter(|addr| {
                    host_allowed || !is_private_ip(&addr.ip()) || policy.allows_ip(&addr.ip())
                })
                .collect();

            if kept.is_empty() {
                return Err(
                    Box::new(BlockedResolution(host)) as Box<dyn std::error::Error + Send + Sync>
                );
            }

            Ok(Box::new(kept.into_iter()) as Addrs)
        })
    }
}

/// A hostname whose every address was refused (DNS rebinding).
#[derive(Debug)]
struct BlockedResolution(String);

impl std::fmt::Display for BlockedResolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} resolves only to addresses this server may not reach",
            self.0
        )
    }
}

impl std::error::Error for BlockedResolution {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// reqwest wraps redirect-policy errors, so assertions read the source chain.
    fn chain(err: &reqwest::Error) -> String {
        let mut out = err.to_string();
        let mut source = std::error::Error::source(err);
        while let Some(e) = source {
            out.push_str(" <- ");
            out.push_str(&e.to_string());
            source = e.source();
        }
        out
    }

    fn resolver(raw: &str) -> GuardedResolver {
        GuardedResolver {
            policy: Arc::new(FetchPolicy::parse(raw).unwrap()),
        }
    }

    async fn resolve(resolver: &GuardedResolver, host: &str) -> Result<Vec<SocketAddr>, String> {
        let name: Name = host.parse().expect("valid DNS name");
        resolver
            .resolve(name)
            .await
            .map(Iterator::collect)
            .map_err(|e| e.to_string())
    }

    /// `localhost` stands in for rebinding: a name resolving to a refused address
    /// (via the hosts file, no network).
    #[tokio::test]
    async fn refuses_a_name_that_resolves_inward() {
        let err = resolve(&resolver(""), "localhost")
            .await
            .expect_err("loopback answers must be dropped");
        assert!(err.contains("may not reach"), "got: {err}");
    }

    #[tokio::test]
    async fn allows_a_name_the_policy_named() {
        let addrs = resolve(&resolver("localhost"), "localhost")
            .await
            .expect("an allowed host keeps its answers");
        assert!(!addrs.is_empty());
    }

    #[tokio::test]
    async fn allows_a_name_resolving_into_an_allowed_network() {
        // The name is not in the allow list; the address it resolves to is.
        let addrs = resolve(&resolver("127.0.0.0/8"), "localhost")
            .await
            .expect("an allowed network keeps its answers");
        assert!(addrs.iter().any(|a| a.ip().is_loopback()));
    }

    /// A name resolving to public and private addresses keeps only the public one.
    #[test]
    fn keeps_only_the_addresses_the_policy_accepts() {
        let policy = FetchPolicy::default();
        let addrs = [
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)), 0),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 0),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)), 0),
        ];

        let kept: Vec<_> = addrs
            .into_iter()
            .filter(|addr| !is_private_ip(&addr.ip()) || policy.allows_ip(&addr.ip()))
            .collect();

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].ip(), IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)));
    }

    #[test]
    fn a_default_fetcher_allows_nothing_private() {
        let fetcher = Fetcher::default();
        assert!(
            fetcher
                .validate(&Url::parse("https://example.com/feed.xml").unwrap())
                .is_ok()
        );
        assert!(
            fetcher
                .validate(&Url::parse("http://10.0.0.1/feed.xml").unwrap())
                .is_err()
        );
    }

    /// The vetted URL is public; only its `Location` points inward.
    #[tokio::test]
    async fn refuses_to_follow_a_redirect_pointing_inward() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/feed.xml"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", "http://169.254.169.254/latest/meta-data/"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let fetcher = Fetcher::new(FetchPolicy::parse("127.0.0.1").unwrap()).unwrap();
        let err = fetcher
            .client(false)
            .get(format!("{}/feed.xml", server.uri()))
            .send()
            .await
            .expect_err("a redirect into a blocked range must not be followed");

        let chain = chain(&err);
        assert!(
            chain.contains("refused to follow a redirect"),
            "got: {chain}"
        );
    }

    /// An in-bounds hop is still followed.
    #[tokio::test]
    async fn follows_a_redirect_that_stays_in_bounds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/feed.xml"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/real.xml"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/real.xml"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<rss/>"))
            .mount(&server)
            .await;

        let fetcher = Fetcher::new(FetchPolicy::parse("127.0.0.1").unwrap()).unwrap();
        let body = fetcher
            .client(false)
            .get(format!("{}/feed.xml", server.uri()))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();

        assert_eq!(body, "<rss/>");
    }

    #[tokio::test]
    async fn stops_a_redirect_loop_at_the_cap() {
        let server = MockServer::start().await;
        // Every hop is in bounds, so only the cap can end this.
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/next"))
            .mount(&server)
            .await;

        let fetcher = Fetcher::new(FetchPolicy::parse("127.0.0.1").unwrap()).unwrap();
        let err = fetcher
            .client(false)
            .get(format!("{}/start", server.uri()))
            .send()
            .await
            .expect_err("an endless redirect chain must stop");

        let chain = chain(&err);
        assert!(chain.contains("too many redirects"), "got: {chain}");
        // Count hops served, since the message also matches reqwest's own cap.
        let served = server.received_requests().await.unwrap().len();
        assert!(
            served <= MAX_REDIRECTS + 1,
            "server saw {served} requests, expected at most {}",
            MAX_REDIRECTS + 1
        );
    }

    #[test]
    fn the_http1_only_client_is_a_separate_pool() {
        let fetcher = Fetcher::default();
        assert!(!std::ptr::eq(fetcher.client(true), fetcher.client(false)));
    }
}
