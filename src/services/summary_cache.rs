use chrono::{DateTime, Utc};
use moka::sync::Cache;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

// Re-export SummaryStatus from models for backward compatibility
pub use crate::models::entry_summary::SummaryStatus;

#[derive(Debug, Clone, Serialize)]
pub struct SummaryCacheEntry {
    pub status: SummaryStatus,
    pub summary_text: Option<String>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl SummaryCacheEntry {
    pub fn new_pending() -> Self {
        Self {
            status: SummaryStatus::Pending,
            summary_text: None,
            error_message: None,
            created_at: Utc::now(),
        }
    }

    pub fn new_processing() -> Self {
        Self {
            status: SummaryStatus::Processing,
            summary_text: None,
            error_message: None,
            created_at: Utc::now(),
        }
    }

    pub fn new_completed(summary_text: String) -> Self {
        Self {
            status: SummaryStatus::Completed,
            summary_text: Some(summary_text),
            error_message: None,
            created_at: Utc::now(),
        }
    }

    pub fn new_failed(error: String) -> Self {
        Self {
            status: SummaryStatus::Failed,
            summary_text: None,
            error_message: Some(error),
            created_at: Utc::now(),
        }
    }
}

type CacheKey = (i64, i64);

#[derive(Clone)]
pub struct SummaryCache {
    cache: Cache<CacheKey, SummaryCacheEntry>,
}

impl SummaryCache {
    /// `ttl_hours` is the time-to-live applied to every entry.
    pub fn new(max_capacity: u64, ttl_hours: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(Duration::from_secs(ttl_hours * 3600))
            .build();

        Self { cache }
    }

    pub fn get(&self, user_id: i64, entry_id: i64) -> Option<SummaryCacheEntry> {
        self.cache.get(&(user_id, entry_id))
    }

    pub fn set_pending(&self, user_id: i64, entry_id: i64) {
        self.cache
            .insert((user_id, entry_id), SummaryCacheEntry::new_pending());
    }

    pub fn set_processing(&self, user_id: i64, entry_id: i64) {
        self.cache
            .insert((user_id, entry_id), SummaryCacheEntry::new_processing());
    }

    pub fn set_completed(&self, user_id: i64, entry_id: i64, text: String) {
        self.cache
            .insert((user_id, entry_id), SummaryCacheEntry::new_completed(text));
    }

    pub fn set_failed(&self, user_id: i64, entry_id: i64, error: String) {
        self.cache
            .insert((user_id, entry_id), SummaryCacheEntry::new_failed(error));
    }

    pub fn remove(&self, user_id: i64, entry_id: i64) {
        self.cache.invalidate(&(user_id, entry_id));
    }

    pub fn get_status(&self, user_id: i64, entry_id: i64) -> Option<SummaryStatus> {
        self.cache.get(&(user_id, entry_id)).map(|e| e.status)
    }
}

pub fn create_summary_cache(max_capacity: u64, ttl_hours: u64) -> Arc<SummaryCache> {
    Arc::new(SummaryCache::new(max_capacity, ttl_hours))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_basic_operations() {
        let cache = SummaryCache::new(100, 24);

        // Initially empty
        assert!(cache.get(1, 100).is_none());

        cache.set_pending(1, 100);
        let entry = cache.get(1, 100).unwrap();
        assert_eq!(entry.status, SummaryStatus::Pending);

        cache.set_processing(1, 100);
        let entry = cache.get(1, 100).unwrap();
        assert_eq!(entry.status, SummaryStatus::Processing);

        cache.set_completed(1, 100, "Test summary".to_string());
        let entry = cache.get(1, 100).unwrap();
        assert_eq!(entry.status, SummaryStatus::Completed);
        assert_eq!(entry.summary_text.as_deref(), Some("Test summary"));

        // Remove
        cache.remove(1, 100);
        assert!(cache.get(1, 100).is_none());
    }

    #[test]
    fn test_cache_failed_status() {
        let cache = SummaryCache::new(100, 24);

        cache.set_failed(1, 100, "Error occurred".to_string());
        let entry = cache.get(1, 100).unwrap();
        assert_eq!(entry.status, SummaryStatus::Failed);
        assert_eq!(entry.error_message.as_deref(), Some("Error occurred"));
    }
}
