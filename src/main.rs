use std::sync::Arc;

use rdrs::{auth, create_router, db, services, AppState, Config, DbPool};
use rusqlite::Connection;
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

    let db = DbPool::new(conn);

    let webauthn = auth::create_webauthn(&config).expect("Failed to create WebAuthn");

    // Create summary cache (max 1000 entries, 24 hour TTL)
    let summary_cache = services::create_summary_cache(1000, 24);

    // Create summary worker channel (buffer size 100)
    let (summary_tx, summary_rx) = services::create_summary_channel(100);

    // Start summary worker
    services::start_summary_worker(summary_rx, summary_cache.clone(), db.clone());

    // Recover incomplete summary jobs from database
    let recovered =
        services::recover_incomplete_jobs(db.clone(), summary_tx.clone(), summary_cache.clone())
            .await;
    if recovered > 0 {
        tracing::info!("Recovered {} incomplete summary jobs", recovered);
    }

    // Start summary cleanup worker (every 1 hour, delete summaries older than 24 hours)
    services::start_cleanup_worker(db.clone(), 1, 24);

    let state = AppState {
        db: db.clone(),
        config: Arc::new(config.clone()),
        webauthn: Arc::new(webauthn),
        summary_cache,
        summary_tx,
    };

    // Start background sync task
    let _background_task = services::start_background_sync(db, config.user_agent.clone());

    let app = create_router(state);

    let addr = format!("0.0.0.0:{}", config.server_port);
    tracing::info!("Starting server on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind");

    axum::serve(listener, app).await.expect("Server failed");
}
