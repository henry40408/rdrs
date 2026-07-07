use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Maximum wall-clock time for a single Kagi summarization request. A hung
/// request would otherwise occupy the single worker indefinitely and block
/// every user's queued summaries. NEVER lower this in production without
/// confirming Kagi's worst-case latency.
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(90);

/// Result of racing a summarization future against cancellation + timeout.
pub(crate) enum SummaryOutcome {
    Completed(String),
    Failed(String),
    Cancelled,
}

/// Race a summarization future against an external cancellation token and a
/// hard timeout. `biased` makes cancellation win deterministically when the
/// token is already cancelled. On timeout the future is dropped (the in-flight
/// HTTP request is aborted) and a `Failed("Summarization timed out")` is
/// returned.
pub(crate) async fn run_summary<F>(
    token: &CancellationToken,
    timeout: Duration,
    fut: F,
) -> SummaryOutcome
where
    F: std::future::Future<Output = Result<String, String>>,
{
    tokio::select! {
        biased;
        _ = token.cancelled() => SummaryOutcome::Cancelled,
        res = tokio::time::timeout(timeout, fut) => match res {
            Ok(Ok(text)) => SummaryOutcome::Completed(text),
            Ok(Err(e)) => SummaryOutcome::Failed(e),
            Err(_elapsed) => SummaryOutcome::Failed("Summarization timed out".to_string()),
        }
    }
}

use super::sidebar_cache::SidebarCache;
use super::summarize::kagi::{self, KagiConfig};
use super::summary_cache::SummaryCache;
use crate::db::Db;
use crate::models::{entry_summary, user_settings};
use crate::services::{EventBus, SummaryStatus};

/// A job to summarize an entry
#[derive(Debug, Clone)]
pub struct SummaryJob {
    pub user_id: i64,
    pub entry_id: i64,
    pub entry_link: String,
}

/// Per-entry cancellation tokens for in-flight / queued summary jobs, keyed by
/// `(user_id, entry_id)`. The cancel handler cancels + removes the token; the
/// worker creates one on dequeue (if absent) and removes it when the job ends.
pub type CancelRegistry = Arc<Mutex<HashMap<(i64, i64), CancellationToken>>>;

/// Start the summary worker that processes jobs from the queue
pub fn start_summary_worker(
    mut rx: mpsc::Receiver<SummaryJob>,
    cache: Arc<SummaryCache>,
    sidebar_cache: Arc<SidebarCache>,
    db: Db,
    cancels: CancelRegistry,
    cancel_token: CancellationToken,
    events: EventBus,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Background priority: DB operations yield to interactive work on SQLite.
        let db = db.background();
        tracing::info!("Summary worker started");

        loop {
            let job = tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("Summary worker stopping, draining remaining jobs...");
                    // Drain remaining jobs before exiting
                    while let Ok(job) = rx.try_recv() {
                        process_summary_job(&job, &cache, &sidebar_cache, &db, &cancels, &events).await;
                    }
                    break;
                }
                job = rx.recv() => {
                    match job {
                        Some(job) => job,
                        None => break,
                    }
                }
            };

            process_summary_job(&job, &cache, &sidebar_cache, &db, &cancels, &events).await;
        }

        tracing::info!("Summary worker stopped");
    })
}

async fn process_summary_job(
    job: &SummaryJob,
    cache: &Arc<SummaryCache>,
    sidebar_cache: &Arc<SidebarCache>,
    db: &Db,
    cancels: &CancelRegistry,
    events: &EventBus,
) {
    let key = (job.user_id, job.entry_id);

    // Get-or-create this job's cancellation token. Covers startup-recovered
    // jobs too (they never pass through the enqueue handler).
    let token = {
        let mut map = cancels.lock().unwrap();
        map.entry(key).or_default().clone()
    };

    // Cancelled while still queued — the cancel handler already deleted the
    // record. Drop the token and skip.
    if token.is_cancelled() {
        cancels.lock().unwrap().remove(&key);
        return;
    }

    run_summary_job_body(job, cache, sidebar_cache, db, &token, events).await;

    cancels.lock().unwrap().remove(&key);
}

async fn run_summary_job_body(
    job: &SummaryJob,
    cache: &Arc<SummaryCache>,
    sidebar_cache: &Arc<SidebarCache>,
    db: &Db,
    token: &CancellationToken,
    events: &EventBus,
) {
    tracing::debug!(
        "Processing summary job: user={}, entry={}, link={}",
        job.user_id,
        job.entry_id,
        job.entry_link
    );

    // Mark as processing in the DB first. If the row no longer exists, the job
    // was cancelled (its record deleted) while it sat in the queue — abort
    // without repopulating the cache, or the cancelled summary would be
    // resurrected from the cache on the next render.
    {
        let user_id = job.user_id;
        let entry_id = job.entry_id;
        if let Err(crate::error::AppError::NotFound(_)) =
            entry_summary::set_processing(db, user_id, entry_id).await
        {
            cache.remove(job.user_id, job.entry_id);
            return;
        }
    }
    cache.set_processing(job.user_id, job.entry_id);
    events.emit_summary(job.user_id, job.entry_id, Some(SummaryStatus::Processing));

    // Get Kagi config for the user
    let user_id = job.user_id;
    let entry_id = job.entry_id;
    let kagi_config = match user_settings::get_save_services_config(db, user_id).await {
        Ok(config) => config.kagi,
        Err(e) => {
            tracing::error!("Failed to get user settings: {}", e);
            let error_msg = "Failed to load Kagi settings".to_string();
            cache.set_failed(job.user_id, job.entry_id, error_msg.clone());
            let _ = entry_summary::set_failed(db, user_id, entry_id, &error_msg).await;
            events.emit_summary(job.user_id, job.entry_id, Some(SummaryStatus::Failed));
            return;
        }
    };

    let kagi_config = match kagi_config {
        Some(c) if c.is_configured() => c,
        _ => {
            let error_msg = "Kagi is not configured".to_string();
            cache.set_failed(job.user_id, job.entry_id, error_msg.clone());
            let user_id = job.user_id;
            let entry_id = job.entry_id;
            let _ = entry_summary::set_failed(db, user_id, entry_id, &error_msg).await;
            events.emit_summary(job.user_id, job.entry_id, Some(SummaryStatus::Failed));
            return;
        }
    };

    // Race the Kagi call against cancellation + timeout.
    match run_summary(
        token,
        SUMMARY_TIMEOUT,
        summarize_with_kagi(&kagi_config, &job.entry_link),
    )
    .await
    {
        SummaryOutcome::Completed(summary_text) => {
            tracing::debug!(
                "Summary completed for entry {}: {} chars",
                job.entry_id,
                summary_text.len()
            );
            let user_id = job.user_id;
            let entry_id = job.entry_id;
            let text = summary_text.clone();
            let db_res = entry_summary::set_completed(db, user_id, entry_id, &text).await;
            if let Err(crate::error::AppError::NotFound(_)) = db_res {
                // Cancelled mid-flight (row deleted) — do not repopulate the cache.
                cache.remove(job.user_id, job.entry_id);
            } else {
                cache.set_completed(job.user_id, job.entry_id, summary_text.clone());
                // A summary just completed — the sidebar "Summarized" badge must tick up.
                sidebar_cache.bust(job.user_id);
                events.emit_summary(job.user_id, job.entry_id, Some(SummaryStatus::Completed));
                events.emit_sidebar(job.user_id);
            }
        }
        SummaryOutcome::Failed(error) => {
            tracing::warn!("Summary failed for entry {}: {}", job.entry_id, error);
            let user_id = job.user_id;
            let entry_id = job.entry_id;
            let err = error.clone();
            let db_res = entry_summary::set_failed(db, user_id, entry_id, &err).await;
            if let Err(crate::error::AppError::NotFound(_)) = db_res {
                // Cancelled mid-flight (row deleted) — do not repopulate the cache.
                cache.remove(job.user_id, job.entry_id);
            } else {
                cache.set_failed(job.user_id, job.entry_id, error.clone());
                events.emit_summary(job.user_id, job.entry_id, Some(SummaryStatus::Failed));
            }
        }
        SummaryOutcome::Cancelled => {
            // The cancel handler owns cleanup (delete + cache remove + sidebar
            // bust). Write nothing back.
            tracing::debug!("Summary cancelled for entry {}", job.entry_id);
        }
    }
}

/// Call Kagi API to get a summary
async fn summarize_with_kagi(config: &KagiConfig, url: &str) -> Result<String, String> {
    match kagi::summarize_url(config, url).await {
        Ok(result) => {
            if result.success {
                result
                    .output_text
                    .ok_or_else(|| "No summary text returned".to_string())
            } else {
                Err(result.error.unwrap_or_else(|| "Unknown error".to_string()))
            }
        }
        Err(e) => Err(format!("Kagi API error: {}", e)),
    }
}

/// Create a summary job queue channel
pub fn create_summary_channel(
    buffer_size: usize,
) -> (mpsc::Sender<SummaryJob>, mpsc::Receiver<SummaryJob>) {
    mpsc::channel(buffer_size)
}

/// Recover incomplete summary jobs on startup
/// Returns the number of jobs re-queued
pub async fn recover_incomplete_jobs(
    db: Db,
    tx: mpsc::Sender<SummaryJob>,
    cache: Arc<SummaryCache>,
) -> usize {
    // Startup recovery is background work; yield to interactive requests.
    let db = db.background();
    let incomplete = match entry_summary::find_incomplete(&db).await {
        Ok(jobs) => jobs,
        Err(e) => {
            tracing::error!("Failed to find incomplete jobs: {}", e);
            return 0;
        }
    };

    let count = incomplete.len();
    if count > 0 {
        tracing::info!("Recovering {} incomplete summary jobs", count);
    }

    for (user_id, entry_id, entry_link) in incomplete {
        // Set pending in cache to track the job
        cache.set_pending(user_id, entry_id);

        let job = SummaryJob {
            user_id,
            entry_id,
            entry_link,
        };

        if let Err(e) = tx.send(job).await {
            tracing::error!("Failed to re-queue job: {}", e);
        }
    }

    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::user::Role;
    use crate::models::{category, entry, entry_summary, feed, user};
    use crate::services::{EventBus, EventKind};

    fn registry() -> CancelRegistry {
        Arc::new(Mutex::new(HashMap::new()))
    }

    async fn setup_test_db() -> Db {
        Db::connect_in_memory().await.unwrap()
    }

    #[test]
    fn test_summary_job_creation() {
        let job = SummaryJob {
            user_id: 1,
            entry_id: 100,
            entry_link: "https://example.com/article".to_string(),
        };

        assert_eq!(job.user_id, 1);
        assert_eq!(job.entry_id, 100);
        assert_eq!(job.entry_link, "https://example.com/article");
    }

    #[test]
    fn test_summary_job_clone() {
        let job = SummaryJob {
            user_id: 1,
            entry_id: 100,
            entry_link: "https://example.com/article".to_string(),
        };

        let cloned = job.clone();
        assert_eq!(cloned.user_id, job.user_id);
        assert_eq!(cloned.entry_id, job.entry_id);
        assert_eq!(cloned.entry_link, job.entry_link);
    }

    #[test]
    fn test_summary_job_debug() {
        let job = SummaryJob {
            user_id: 1,
            entry_id: 100,
            entry_link: "https://example.com/article".to_string(),
        };

        let debug_str = format!("{:?}", job);
        assert!(debug_str.contains("SummaryJob"));
        assert!(debug_str.contains("user_id: 1"));
        assert!(debug_str.contains("entry_id: 100"));
    }

    #[tokio::test]
    async fn test_channel_creation() {
        let (tx, mut rx) = create_summary_channel(10);

        let job = SummaryJob {
            user_id: 1,
            entry_id: 100,
            entry_link: "https://example.com".to_string(),
        };

        tx.send(job.clone()).await.unwrap();
        let received = rx.recv().await.unwrap();

        assert_eq!(received.user_id, job.user_id);
        assert_eq!(received.entry_id, job.entry_id);
        assert_eq!(received.entry_link, job.entry_link);
    }

    #[tokio::test]
    async fn test_worker_stops_on_cancellation() {
        let (tx, rx) = create_summary_channel(10);
        let cache = Arc::new(SummaryCache::new(100, 24));
        let db = setup_test_db().await;
        let cancel_token = CancellationToken::new();

        let handle = start_summary_worker(
            rx,
            cache,
            Arc::new(SidebarCache::default()),
            db,
            registry(),
            cancel_token.clone(),
            EventBus::new(8),
        );

        // Send a job (it won't be processed properly without Kagi config, but that's OK)
        let _ = tx
            .send(SummaryJob {
                user_id: 1,
                entry_id: 1,
                entry_link: "https://example.com".to_string(),
            })
            .await;

        // Cancel the worker
        cancel_token.cancel();

        // Worker should stop
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        assert!(result.is_ok(), "Worker should stop after cancellation");
    }

    #[tokio::test]
    async fn test_worker_stops_when_channel_closed() {
        let (tx, rx) = create_summary_channel(10);
        let cache = Arc::new(SummaryCache::new(100, 24));
        let db = setup_test_db().await;
        let cancel_token = CancellationToken::new();

        let handle = start_summary_worker(
            rx,
            cache,
            Arc::new(SidebarCache::default()),
            db,
            registry(),
            cancel_token,
            EventBus::new(8),
        );

        // Drop the sender to close the channel
        drop(tx);

        // Worker should stop
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        assert!(result.is_ok(), "Worker should stop when channel closes");
    }

    #[tokio::test]
    async fn test_recover_incomplete_jobs_empty() {
        let db = setup_test_db().await;
        let (tx, _rx) = create_summary_channel(10);
        let cache = Arc::new(SummaryCache::new(100, 24));

        // No incomplete jobs to recover
        let count = recover_incomplete_jobs(db, tx, cache).await;
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_recover_incomplete_jobs_with_pending() {
        let db = setup_test_db().await;

        // Create test data with a pending summary
        let user_id = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap()
            .id;
        let category_id = category::create_category(&db, user_id, "Tech")
            .await
            .unwrap()
            .id;
        let feed_id = feed::create_feed(
            &db,
            &feed::CreateFeedParams {
                category_id,
                url: "https://example.com/feed.xml",
                title: Some("Feed"),
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap()
        .id;

        let (entry_obj, _) = entry::upsert_entry(
            &db,
            feed_id,
            "guid-1",
            Some("Entry"),
            Some("https://example.com/article"),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // Create a pending summary
        entry_summary::upsert_pending(&db, user_id, entry_obj.id)
            .await
            .unwrap();

        let (tx, mut rx) = create_summary_channel(10);
        let cache = Arc::new(SummaryCache::new(100, 24));

        let count = recover_incomplete_jobs(db, tx, cache.clone()).await;
        assert_eq!(count, 1);

        // Verify job was queued
        let job = rx.try_recv().unwrap();
        assert_eq!(job.entry_link, "https://example.com/article");

        // Verify cache was updated
        let status = cache.get(job.user_id, job.entry_id);
        assert!(status.is_some());
    }

    #[tokio::test]
    async fn test_recover_incomplete_jobs_with_processing() {
        let db = setup_test_db().await;

        // Create test data with a processing (stuck) summary
        let user_id = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap()
            .id;
        let category_id = category::create_category(&db, user_id, "Tech")
            .await
            .unwrap()
            .id;
        let feed_id = feed::create_feed(
            &db,
            &feed::CreateFeedParams {
                category_id,
                url: "https://example.com/feed.xml",
                title: Some("Feed"),
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap()
        .id;

        let (entry_obj, _) = entry::upsert_entry(
            &db,
            feed_id,
            "guid-2",
            Some("Entry 2"),
            Some("https://example.com/article2"),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // Create a processing summary (simulates a crashed worker)
        entry_summary::upsert_pending(&db, user_id, entry_obj.id)
            .await
            .unwrap();
        entry_summary::set_processing(&db, user_id, entry_obj.id)
            .await
            .unwrap();

        let (tx, mut rx) = create_summary_channel(10);
        let cache = Arc::new(SummaryCache::new(100, 24));

        let count = recover_incomplete_jobs(db, tx, cache).await;
        assert_eq!(count, 1);

        // Verify job was queued
        let job = rx.try_recv().unwrap();
        assert_eq!(job.entry_link, "https://example.com/article2");
    }

    #[tokio::test]
    async fn run_summary_completes() {
        let token = CancellationToken::new();
        let out = run_summary(&token, std::time::Duration::from_secs(1), async {
            Ok::<String, String>("hello".to_string())
        })
        .await;
        assert!(matches!(out, SummaryOutcome::Completed(s) if s == "hello"));
    }

    #[tokio::test]
    async fn run_summary_propagates_failure() {
        let token = CancellationToken::new();
        let out = run_summary(&token, std::time::Duration::from_secs(1), async {
            Err::<String, String>("boom".to_string())
        })
        .await;
        assert!(matches!(out, SummaryOutcome::Failed(e) if e == "boom"));
    }

    #[tokio::test]
    async fn run_summary_times_out() {
        let token = CancellationToken::new();
        let out = run_summary(&token, std::time::Duration::from_millis(20), async {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            Ok::<String, String>("late".to_string())
        })
        .await;
        assert!(matches!(out, SummaryOutcome::Failed(e) if e == "Summarization timed out"));
    }

    #[tokio::test]
    async fn run_summary_cancels_before_completion() {
        let token = CancellationToken::new();
        token.cancel();
        let out = run_summary(&token, std::time::Duration::from_secs(1), async {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            Ok::<String, String>("never".to_string())
        })
        .await;
        assert!(matches!(out, SummaryOutcome::Cancelled));
    }

    #[tokio::test]
    async fn cancelled_while_queued_does_not_repopulate_cache() {
        let db = setup_test_db().await;

        let u = user::create_user(&db, "canceluser", "hash", Role::User)
            .await
            .unwrap()
            .id;
        let cat = category::create_category(&db, u, "Tech").await.unwrap().id;
        let feed_id = feed::create_feed(
            &db,
            &feed::CreateFeedParams {
                category_id: cat,
                url: "https://example.com/feed.xml",
                title: Some("Feed"),
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap()
        .id;
        let (entry_obj, _) = entry::upsert_entry(
            &db,
            feed_id,
            "guid-cancelled",
            Some("Cancelled Entry"),
            Some("https://example.com/x"),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        // Intentionally do NOT create an entry_summary row — this
        // simulates the cancel handler having already deleted it while
        // the job was still sitting in the queue.
        let (user_id, entry_id) = (u, entry_obj.id);

        let cache = Arc::new(SummaryCache::new(100, 24));
        let sidebar = Arc::new(SidebarCache::default());
        let cancels = registry();

        // Pre-seed the cache as if enqueue set it to pending, to prove the
        // worker removes the stale cache entry rather than promoting it.
        cache.set_pending(user_id, entry_id);

        let job = SummaryJob {
            user_id,
            entry_id,
            entry_link: "https://example.com/x".to_string(),
        };

        process_summary_job(&job, &cache, &sidebar, &db, &cancels, &EventBus::new(8)).await;

        // The set_processing UPDATE hits 0 rows (no summary row exists) ->
        // AppError::NotFound -> worker removes the stale cache entry instead
        // of repopulating it.
        assert!(
            cache.get(user_id, entry_id).is_none(),
            "cache must not be repopulated for a cancelled (row-deleted) job"
        );

        // Confirm no row was resurrected in the DB either.
        let row = entry_summary::find_by_user_and_entry(&db, user_id, entry_id)
            .await
            .unwrap();
        assert!(
            row.is_none(),
            "no entry_summary row must exist after a cancelled job"
        );
    }

    #[tokio::test]
    async fn test_worker_drains_jobs_on_cancellation() {
        let (tx, rx) = create_summary_channel(10);
        let cache = Arc::new(SummaryCache::new(100, 24));
        let db = setup_test_db().await;
        let cancel_token = CancellationToken::new();

        // Create test user for the jobs
        user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        let handle = start_summary_worker(
            rx,
            cache.clone(),
            Arc::new(SidebarCache::default()),
            db,
            registry(),
            cancel_token.clone(),
            EventBus::new(8),
        );

        // Send multiple jobs
        for i in 1..=3 {
            tx.send(SummaryJob {
                user_id: 1,
                entry_id: i,
                entry_link: format!("https://example.com/{}", i),
            })
            .await
            .unwrap();
        }

        // Give worker a moment to start processing
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Cancel the worker
        cancel_token.cancel();

        // Worker should stop after draining
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        assert!(result.is_ok(), "Worker should stop after draining jobs");
    }

    #[tokio::test]
    async fn worker_emits_processing_then_terminal_event() {
        let (tx, rx) = create_summary_channel(10);
        let cache = Arc::new(SummaryCache::new(100, 24));
        let db = setup_test_db().await;
        let cancel_token = CancellationToken::new();
        let bus = EventBus::new(32);
        let mut sub = bus.subscribe();

        // Seed a user + entry + pending summary so set_processing finds a row.
        let u = user::create_user(&db, "emit", "hash", Role::User)
            .await
            .unwrap()
            .id;
        let cat = category::create_category(&db, u, "Tech").await.unwrap().id;
        let feed_id = feed::create_feed(
            &db,
            &feed::CreateFeedParams {
                category_id: cat,
                url: "https://example.com/feed.xml",
                title: Some("F"),
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap()
        .id;
        let (e, _) = entry::upsert_entry(
            &db,
            feed_id,
            "g",
            Some("T"),
            Some("https://example.com/a"),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        entry_summary::upsert_pending(&db, u, e.id).await.unwrap();
        let (user_id, entry_id) = (u, e.id);

        let handle = start_summary_worker(
            rx,
            cache,
            Arc::new(SidebarCache::default()),
            db,
            registry(),
            cancel_token.clone(),
            bus,
        );
        tx.send(SummaryJob {
            user_id,
            entry_id,
            entry_link: "https://example.com/a".into(),
        })
        .await
        .unwrap();

        // First event must be Summary{Processing} for this entry. (Kagi is not
        // configured in tests, so the job then fails — we assert only the
        // processing emission, which is deterministic.)
        let ev = tokio::time::timeout(std::time::Duration::from_secs(3), sub.recv())
            .await
            .expect("an event should be emitted")
            .unwrap();
        assert_eq!(ev.user_id, user_id);
        assert!(matches!(
            ev.kind,
            EventKind::Summary { entry_id: e, status: Some(SummaryStatus::Processing) } if e == entry_id
        ));

        cancel_token.cancel();
        let _ = handle.await;
    }
}
