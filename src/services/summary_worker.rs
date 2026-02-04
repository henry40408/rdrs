use std::sync::Arc;

use tokio::sync::mpsc;

use super::summarize::kagi::{self, KagiConfig};
use super::summary_cache::SummaryCache;
use crate::db::DbPool;
use crate::models::{entry_summary, user_settings};

/// A job to summarize an entry
#[derive(Debug, Clone)]
pub struct SummaryJob {
    pub user_id: i64,
    pub entry_id: i64,
    pub entry_link: String,
}

/// Start the summary worker that processes jobs from the queue
pub fn start_summary_worker(
    mut rx: mpsc::Receiver<SummaryJob>,
    cache: Arc<SummaryCache>,
    db: DbPool,
) {
    tokio::spawn(async move {
        tracing::info!("Summary worker started");

        while let Some(job) = rx.recv().await {
            tracing::debug!(
                "Processing summary job: user={}, entry={}, link={}",
                job.user_id,
                job.entry_id,
                job.entry_link
            );

            // Mark as processing in both cache and DB
            cache.set_processing(job.user_id, job.entry_id);
            {
                let user_id = job.user_id;
                let entry_id = job.entry_id;
                let _ = db
                    .background(move |conn| entry_summary::set_processing(conn, user_id, entry_id))
                    .await;
            }

            // Get Kagi config for the user
            let user_id = job.user_id;
            let entry_id = job.entry_id;
            let kagi_config = match db
                .background(move |conn| user_settings::get_save_services_config(conn, user_id))
                .await
            {
                Ok(Ok(config)) => config.kagi,
                Ok(Err(e)) => {
                    tracing::error!("Failed to get user settings: {}", e);
                    let error_msg = "Failed to load Kagi settings".to_string();
                    cache.set_failed(job.user_id, job.entry_id, error_msg.clone());
                    let _ = db
                        .background(move |conn| {
                            entry_summary::set_failed(conn, user_id, entry_id, &error_msg)
                        })
                        .await;
                    continue;
                }
                Err(e) => {
                    tracing::error!("Failed to access DB: {}", e);
                    let error_msg = "Internal error: DB access failed".to_string();
                    cache.set_failed(job.user_id, job.entry_id, error_msg);
                    continue;
                }
            };

            let kagi_config = match kagi_config {
                Some(c) if c.is_configured() => c,
                _ => {
                    let error_msg = "Kagi is not configured".to_string();
                    cache.set_failed(job.user_id, job.entry_id, error_msg.clone());
                    let user_id = job.user_id;
                    let entry_id = job.entry_id;
                    let _ = db
                        .background(move |conn| {
                            entry_summary::set_failed(conn, user_id, entry_id, &error_msg)
                        })
                        .await;
                    continue;
                }
            };

            // Call Kagi API
            match summarize_with_kagi(&kagi_config, &job.entry_link).await {
                Ok(summary_text) => {
                    tracing::debug!(
                        "Summary completed for entry {}: {} chars",
                        job.entry_id,
                        summary_text.len()
                    );
                    // Update cache
                    cache.set_completed(job.user_id, job.entry_id, summary_text.clone());
                    // Update DB
                    let user_id = job.user_id;
                    let entry_id = job.entry_id;
                    let _ = db
                        .background(move |conn| {
                            entry_summary::set_completed(conn, user_id, entry_id, &summary_text)
                        })
                        .await;
                }
                Err(error) => {
                    tracing::warn!("Summary failed for entry {}: {}", job.entry_id, error);
                    cache.set_failed(job.user_id, job.entry_id, error.clone());
                    // Update DB
                    let user_id = job.user_id;
                    let entry_id = job.entry_id;
                    let _ = db
                        .background(move |conn| {
                            entry_summary::set_failed(conn, user_id, entry_id, &error)
                        })
                        .await;
                }
            }
        }

        tracing::info!("Summary worker stopped");
    });
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
    db: DbPool,
    tx: mpsc::Sender<SummaryJob>,
    cache: Arc<SummaryCache>,
) -> usize {
    let incomplete = match db.background(entry_summary::find_incomplete).await {
        Ok(Ok(jobs)) => jobs,
        Ok(Err(e)) => {
            tracing::error!("Failed to find incomplete jobs: {}", e);
            return 0;
        }
        Err(e) => {
            tracing::error!("Failed to access DB for recovery: {}", e);
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
}
