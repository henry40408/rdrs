use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

// Use mimalloc to keep resident memory low: long-running multi-threaded sync
// accumulates allocator fragmentation that the system glibc allocator tends to
// retain as RSS. mimalloc returns freed pages to the OS far more aggressively.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use clap::Parser;
use rdrs::{AppState, Config, auth, create_router, db::Db, services};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{
    EnvFilter, Layer as _, fmt::format::FmtSpan, layer::SubscriberExt, util::SubscriberInitExt,
};

#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
enum LogFormat {
    #[default]
    Full,
    Compact,
    Pretty,
    Json,
}

#[derive(Parser, Debug)]
#[command(name = "rdrs")]
struct Args {
    /// Log output format
    #[arg(long, env = "RDRS_LOG_FORMAT", default_value = "full")]
    log_format: LogFormat,
}

fn init_tracing(format: LogFormat) {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("error,rdrs=info"));
    let span_events = env_filter.max_level_hint().map_or(FmtSpan::CLOSE, |l| {
        if l >= tracing::Level::DEBUG {
            FmtSpan::CLOSE
        } else {
            FmtSpan::NONE
        }
    });
    let use_ansi = std::env::var_os("NO_COLOR").is_none();
    let layer = tracing_subscriber::fmt::layer()
        .with_span_events(span_events)
        .with_ansi(use_ansi);
    let layer = match format {
        LogFormat::Full => layer.with_filter(env_filter).boxed(),
        LogFormat::Compact => layer.compact().with_filter(env_filter).boxed(),
        LogFormat::Pretty => layer.pretty().with_filter(env_filter).boxed(),
        LogFormat::Json => layer.json().with_filter(env_filter).boxed(),
    };
    tracing_subscriber::registry().with(layer).init();
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    init_tracing(args.log_format);

    let config = Config::from_env().unwrap_or_else(|msg| {
        eprintln!("Configuration error: {msg}");
        std::process::exit(1);
    });

    if let Err(msg) = config.validate() {
        eprintln!("Configuration error: {msg}");
        std::process::exit(1);
    }

    if config.secret_generated {
        tracing::warn!(
            event = "config.warning",
            kind = "ephemeral_secret",
            "RDRS_SECRET is not set or too short; using a key generated for this process only. \
             Every signed-in browser session ends on restart, and image-proxy URLs already cached \
             by Google Reader clients break until the next sync. Set RDRS_SECRET (e.g. \
             `openssl rand -base64 32`) to keep them stable across restarts."
        );
    }

    if let Some(warning) = config.webauthn_rp_warning() {
        tracing::warn!(event = "config.warning", kind = "webauthn_rp", "{warning}");
    }

    if let Some(warning) = config.rate_limit_proxy_warning() {
        tracing::warn!(
            event = "config.warning",
            kind = "rate_limit_proxy",
            "{warning}"
        );
    }

    // Open the pool for the configured backend and run its migrations. The
    // backend is fixed for the process lifetime (see `Config::backend`).
    let db = Db::connect(&config.database_url, config.backend())
        .await
        .expect("Failed to open database");

    let webauthn = auth::create_webauthn(&config).expect("Failed to create WebAuthn");

    let cancel_token = CancellationToken::new();

    // Event bus for SSE live updates (sidebar + summary). Capacity covers a
    // burst of mutations without lagging a slow subscriber; a lagged receiver
    // recovers via a sidebar resync signal.
    let events = services::EventBus::new(256);

    let summary_cache = services::create_summary_cache(1000, 24);

    let (summary_tx, summary_rx) = services::create_summary_channel(100);

    let sidebar_cache = Arc::new(services::SidebarCache::default());

    // Per-entry cancellation tokens for summary jobs (cancel/abort support)
    let summary_cancels: rdrs::services::CancelRegistry =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    let summary_worker_handle = services::start_summary_worker(
        summary_rx,
        summary_cache.clone(),
        sidebar_cache.clone(),
        db.clone(),
        summary_cancels.clone(),
        cancel_token.clone(),
        events.clone(),
    );

    // Recover incomplete summary jobs from database
    let recovered =
        services::recover_incomplete_jobs(db.clone(), summary_tx.clone(), summary_cache.clone())
            .await;
    if recovered > 0 {
        tracing::info!(
            event = "summary.recovered",
            count = recovered,
            "recovered incomplete summary jobs"
        );
    }

    // Start summary cleanup worker (every 1 hour, delete summaries older than 24 hours)
    let cleanup_worker_handle =
        services::start_cleanup_worker(db.clone(), 1, 24, cancel_token.clone());

    // Start read-entry retention worker (every 24h; per-user opt-in via
    // user_settings.retention_read_days, 0 = disabled). No-op when nobody opted in.
    let retention_worker_handle =
        services::start_retention_worker(db.clone(), 24, cancel_token.clone());

    // Backfill entry.content_text for rows predating migration v10 in the
    // background so startup is not blocked. Idempotent and one-shot: a
    // fully-backfilled DB costs a single COUNT and the task exits. Body search
    // over not-yet-filled rows is degraded until it completes.
    let content_text_backfill_handle =
        services::start_content_text_backfill(db.clone(), cancel_token.clone());

    let state = AppState {
        db: db.clone(),
        config: Arc::new(config.clone()),
        webauthn: Arc::new(webauthn),
        summary_cache,
        summary_tx,
        sidebar_cache: sidebar_cache.clone(),
        summary_cancels,
        summarizer_inflight: rdrs::handlers::summarizer::new_inflight_registry(),
        events: events.clone(),
        shutdown: cancel_token.clone(),
        login_rate_limiter: Arc::new(rdrs::middleware::RateLimiter::new(
            config.login_rate_limit_attempts,
            config.login_rate_limit_window_secs,
        )),
    };

    let background_handle = services::start_background_sync(
        db.clone(),
        config.user_agent.clone(),
        config.fetch_allow_private.clone(),
        cancel_token.clone(),
        sidebar_cache.clone(),
        events.clone(),
    );

    let app = create_router(state);

    let addr = config.server_bind;
    tracing::info!(event = "server.starting", addr = %addr, "starting server");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind");

    // Start server with graceful shutdown. Cancelling the token from inside
    // the shutdown future ends every in-flight SSE stream so the server does
    // not hang waiting on long-lived connections.
    let shutdown_token = cancel_token.clone();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        shutdown_token.cancel();
    })
    .await
    .expect("Server failed");

    tracing::info!(
        event = "server.stopped",
        "server stopped, initiating graceful shutdown"
    );

    // Cancel background tasks (idempotent — already cancelled above).
    cancel_token.cancel();

    tracing::info!(
        event = "shutdown.waiting",
        timeout_s = 30,
        "waiting for background tasks to complete"
    );
    let shutdown_timeout = tokio::time::timeout(Duration::from_secs(30), async {
        let _ = tokio::join!(
            background_handle,
            summary_worker_handle,
            cleanup_worker_handle,
            retention_worker_handle,
            content_text_backfill_handle,
        );
    });

    if shutdown_timeout.await.is_err() {
        tracing::warn!(
            event = "shutdown.timeout",
            timeout_s = 30,
            "background tasks did not complete in time"
        );
    } else {
        tracing::info!(
            event = "shutdown.tasks_completed",
            "all background tasks completed"
        );
    }

    // Shutdown database: checkpoint the WAL (SQLite) and close the pool.
    db.shutdown().await;

    tracing::info!(event = "shutdown.complete", "graceful shutdown complete");
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
        () = ctrl_c => {
            tracing::info!(event = "shutdown.signal", signal = "SIGINT", "received Ctrl+C, shutting down");
        }
        () = terminate => {
            tracing::info!(event = "shutdown.signal", signal = "SIGTERM", "received SIGTERM, shutting down");
        }
    }
}
