use std::sync::Arc;

use rusqlite::Connection;
use std::sync::Mutex;
use tokio::sync::mpsc;

use super::summarize::kagi::{self, KagiConfig};
use super::summary_cache::SummaryCache;
use crate::models::user_settings;

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
    db: Arc<Mutex<Connection>>,
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

            // Mark as processing
            cache.set_processing(job.user_id, job.entry_id);

            // Get Kagi config for the user
            let kagi_config = {
                let conn = match db.lock() {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("Failed to acquire DB lock: {}", e);
                        cache.set_failed(
                            job.user_id,
                            job.entry_id,
                            "Internal error: DB lock failed".to_string(),
                        );
                        continue;
                    }
                };

                match user_settings::get_save_services_config(&conn, job.user_id) {
                    Ok(config) => config.kagi,
                    Err(e) => {
                        tracing::error!("Failed to get user settings: {}", e);
                        cache.set_failed(
                            job.user_id,
                            job.entry_id,
                            "Failed to load Kagi settings".to_string(),
                        );
                        continue;
                    }
                }
            };

            let kagi_config = match kagi_config {
                Some(c) if c.is_configured() => c,
                _ => {
                    cache.set_failed(
                        job.user_id,
                        job.entry_id,
                        "Kagi is not configured".to_string(),
                    );
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
                    cache.set_completed(job.user_id, job.entry_id, summary_text);
                }
                Err(error) => {
                    tracing::warn!("Summary failed for entry {}: {}", job.entry_id, error);
                    cache.set_failed(job.user_id, job.entry_id, error);
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
