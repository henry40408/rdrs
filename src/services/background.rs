use std::sync::{Arc, Mutex};

use chrono::Utc;
use rusqlite::Connection;
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info};

use super::feed_sync;

pub fn start_background_sync(db: Arc<Mutex<Connection>>) -> JoinHandle<()> {
    tokio::spawn(async move {
        info!("Background sync task started");

        let mut ticker = interval(Duration::from_secs(60));

        loop {
            ticker.tick().await;

            let now = Utc::now();
            let bucket = (now.timestamp() / 60 % 60) as u8;

            debug!("Running background sync for bucket {}", bucket);

            let results = feed_sync::refresh_bucket(db.clone(), bucket).await;

            let success_count = results.iter().filter(|(_, r)| r.is_ok()).count();
            let fail_count = results.iter().filter(|(_, r)| r.is_err()).count();

            if !results.is_empty() {
                info!(
                    "Background sync bucket {}: {} succeeded, {} failed",
                    bucket, success_count, fail_count
                );
            }

            for (feed_id, result) in &results {
                if let Err(e) = result {
                    error!("Background sync feed {} failed: {}", feed_id, e);
                }
            }
        }
    })
}
