use std::sync::{Arc, Mutex};

use rdrs::{create_router, db, services, AppState, Config};
use rusqlite::Connection;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::from_env();

    let conn = Connection::open(&config.database_url).expect("Failed to open database");
    db::init_db(&conn).expect("Failed to initialize database");

    let db = Arc::new(Mutex::new(conn));

    let state = AppState {
        db: db.clone(),
        config: Arc::new(config.clone()),
    };

    // Start background sync task
    let _background_task = services::start_background_sync(db);

    let app = create_router(state);

    let addr = format!("0.0.0.0:{}", config.server_port);
    tracing::info!("Starting server on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind");

    axum::serve(listener, app).await.expect("Server failed");
}
