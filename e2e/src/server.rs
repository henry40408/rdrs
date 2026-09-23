//! An rdrs server under test plus its two mock upstreams.
//!
//! The binary is spawned directly, not via `cargo run`, so killing our PID
//! kills the server rather than leaving it holding the port.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::any;
use tokio::task::JoinHandle;

/// How long to wait for `/health` to answer.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// What the mock upstream serves for every feed URL a scenario subscribes to.
const MOCK_RSS_FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Test Feed</title>
    <link>http://localhost</link>
    <description>A test feed for E2E tests</description>
  </channel>
</rss>"#;

/// Mock Kagi delay; summarizer scenarios assert on pending/processing SSE
/// events an instant response would race past.
const KAGI_LATENCY: Duration = Duration::from_millis(300);

/// The addresses a scenario needs; cheap to clone, owns no process.
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// Base URL of the rdrs server under test.
    pub base_url: String,
    /// The `SQLite` file the server was started against, for direct seeding.
    pub db_path: PathBuf,
    /// A URL that always answers with a valid RSS document.
    pub feed_url: String,
}

/// A running rdrs server and its mock upstreams, torn down on drop.
pub struct Harness {
    endpoints: Endpoints,
    child: Child,
    mocks: Vec<JoinHandle<()>>,
    // Held for its Drop: removes the directory containing the test database.
    _temp: tempfile::TempDir,
}

impl Harness {
    /// Builds the binary if needed, starts the mocks and the server, and waits
    /// for `/health`.
    pub async fn start() -> Result<Self> {
        let binary = ensure_binary()?;
        let temp = tempfile::Builder::new()
            .prefix("rdrs-e2e-")
            .tempdir()
            .context("creating the temporary directory for the test database")?;
        let db_path = temp.path().join("test.sqlite3");

        let (feed_url, feed_task) = spawn_mock_feed().await?;
        let (kagi_url, kagi_task) = spawn_mock_kagi().await?;

        let port = free_port()?;
        let base_url = format!("http://127.0.0.1:{port}");

        let child = Command::new(&binary)
            .current_dir(repo_root())
            .env("DATABASE_URL", &db_path)
            .env("RDRS_SERVER_BIND", format!("127.0.0.1:{port}"))
            .env("RDRS_MULTI_USER_ENABLED", "true")
            .env("RUST_LOG", "warn")
            // Minimal Argon2 cost for per-scenario logins. Never in production.
            .env("RDRS_FAST_HASH", "1")
            // All scenarios share 127.0.0.1's bucket; the limiter would refuse
            // logins a few scenarios in. It is covered elsewhere.
            .env("RDRS_LOGIN_RATE_LIMIT_ATTEMPTS", "0")
            // Direct SQLite seeding skips the cache's bust hooks, so a render
            // mid-seed would cache a half-seeded sidebar for the whole TTL.
            // Never in production.
            .env("RDRS_DISABLE_SIDEBAR_CACHE", "1")
            .env("RDRS_KAGI_API_BASE", &kagi_url)
            // The mock feed binds loopback, which the SSRF guard refuses by
            // default; this is the self-hoster LAN opt-in, not a test bypass.
            .env("RDRS_FETCH_ALLOW_PRIVATE_HOSTS", "127.0.0.1")
            // Inherited so startup failures show in the test output.
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("spawning the rdrs server at {}", binary.display()))?;

        // Bound before the wait so a server that never answers is still killed.
        let harness = Self {
            endpoints: Endpoints {
                base_url: base_url.clone(),
                db_path,
                feed_url,
            },
            child,
            mocks: vec![feed_task, kagi_task],
            _temp: temp,
        };
        wait_until_healthy(&base_url).await?;
        Ok(harness)
    }

    /// The addresses scenarios connect to.
    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for task in &self.mocks {
            task.abort();
        }
    }
}

/// Path to the dev-profile server binary, building it if missing. Dev, not
/// release: release is LTO-tuned for Docker and slow to build.
fn ensure_binary() -> Result<PathBuf> {
    let binary = repo_root().join("target/debug/rdrs");
    if binary.is_file() {
        return Ok(binary);
    }

    eprintln!("e2e: {} is missing — building it", binary.display());
    let status = Command::new("cargo")
        .current_dir(repo_root())
        .arg("build")
        .status()
        .context("running `cargo build`")?;
    if !status.success() {
        bail!("`cargo build` failed with {status}");
    }
    if !binary.is_file() {
        bail!("`cargo build` did not produce {}", binary.display());
    }
    Ok(binary)
}

/// Serves a fixed RSS document on every path.
async fn spawn_mock_feed() -> Result<(String, JoinHandle<()>)> {
    let app = Router::new().fallback(any(|| async {
        (
            [(header::CONTENT_TYPE, "application/rss+xml")],
            MOCK_RSS_FEED,
        )
            .into_response()
    }));
    serve(app).await
}

/// Stands in for the Kagi summarisation API.
async fn spawn_mock_kagi() -> Result<(String, JoinHandle<()>)> {
    let app = Router::new().fallback(any(|| async {
        tokio::time::sleep(KAGI_LATENCY).await;
        axum::Json(serde_json::json!({
            "output_data": { "markdown": "E2E mock summary body." }
        }))
        .into_response()
    }));
    serve(app).await
}

/// Binds `app` to an ephemeral port and serves it on a background task.
async fn serve(app: Router) -> Result<(String, JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("binding a mock upstream")?;
    let url = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((url, task))
}

/// An unused TCP port. Racy (released before the server claims it), but with
/// one server per run there is little to collide with.
fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("probing for a free port")?;
    Ok(listener.local_addr()?.port())
}

async fn wait_until_healthy(base_url: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let health = format!("{base_url}/health");
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(response) = client.get(&health).send().await
            && response.status().is_success()
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("the rdrs server did not answer {health} within {STARTUP_TIMEOUT:?}")
}

/// The repository root — the parent of this crate's directory.
fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("e2e/ always has a parent")
}
