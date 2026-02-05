use std::sync::Arc;
use std::time::Duration;

use rdrs::{auth, create_router, db, services, AppState, Config, DbPool};
use rusqlite::Connection;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::from_env();

    if config.image_proxy_secret_generated {
        tracing::warn!(
            "IMAGE_PROXY_SECRET not set, using temporary key. Proxy URLs will be invalidated on restart."
        );
    }

    let conn = Connection::open(&config.database_url).expect("Failed to open database");
    db::init_db(&conn).expect("Failed to initialize database");

    let (db, db_handle) = DbPool::new(conn);

    let webauthn = auth::create_webauthn(&config).expect("Failed to create WebAuthn");

    // Create cancellation token for graceful shutdown
    let cancel_token = CancellationToken::new();

    // Create summary cache (max 1000 entries, 24 hour TTL)
    let summary_cache = services::create_summary_cache(1000, 24);

    // Create summary worker channel (buffer size 100)
    let (summary_tx, summary_rx) = services::create_summary_channel(100);

    // Start summary worker
    let summary_worker_handle = services::start_summary_worker(
        summary_rx,
        summary_cache.clone(),
        db.clone(),
        cancel_token.clone(),
    );

    // Recover incomplete summary jobs from database
    let recovered =
        services::recover_incomplete_jobs(db.clone(), summary_tx.clone(), summary_cache.clone())
            .await;
    if recovered > 0 {
        tracing::info!("Recovered {} incomplete summary jobs", recovered);
    }

    // Start summary cleanup worker (every 1 hour, delete summaries older than 24 hours)
    let cleanup_worker_handle =
        services::start_cleanup_worker(db.clone(), 1, 24, cancel_token.clone());

    let state = AppState {
        db: db.clone(),
        config: Arc::new(config.clone()),
        webauthn: Arc::new(webauthn),
        summary_cache,
        summary_tx,
    };

    // Start background sync task
    let background_handle =
        services::start_background_sync(db.clone(), config.user_agent.clone(), cancel_token.clone());

    let app = create_router(state);

    let addr = format!("0.0.0.0:{}", config.server_port);
    tracing::info!("Starting server on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind");

    // Start server with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("Server failed");

    tracing::info!("Server stopped, initiating graceful shutdown...");

    // Cancel background tasks
    cancel_token.cancel();

    // Wait for background tasks to complete (with timeout)
    tracing::info!("Waiting for background tasks to complete...");
    let shutdown_timeout = tokio::time::timeout(Duration::from_secs(30), async {
        let _ = tokio::join!(
            background_handle,
            summary_worker_handle,
            cleanup_worker_handle,
        );
    });

    if shutdown_timeout.await.is_err() {
        tracing::warn!("Background tasks did not complete within 30 seconds");
    } else {
        tracing::info!("All background tasks completed");
    }

    // Shutdown database (execute WAL checkpoint)
    if let Err(e) = db.shutdown().await {
        tracing::error!("Failed to shutdown database cleanly: {}", e);
    }

    // Wait for database actor to exit
    if db_handle.await.is_err() {
        tracing::warn!("Database actor task panicked");
    }

    tracing::info!("Graceful shutdown complete");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received Ctrl+C, shutting down...");
        }
        _ = terminate => {
            tracing::info!("Received SIGTERM, shutting down...");
        }
    }
}
