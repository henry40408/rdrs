use chrono::{DateTime, Utc};
use moka::sync::Cache;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

/// Summary processing status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SummaryStatus {
    Pending,
    Processing,
    Completed,
    Failed,
}

/// A cached summary entry
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

/// Cache key: (user_id, entry_id)
type CacheKey = (i64, i64);

/// LRU cache for summaries with TTL support
#[derive(Clone)]
pub struct SummaryCache {
    cache: Cache<CacheKey, SummaryCacheEntry>,
}

impl SummaryCache {
    /// Create a new summary cache
    ///
    /// # Arguments
    /// * `max_capacity` - Maximum number of entries to store
    /// * `ttl_hours` - Time-to-live in hours for each entry
    pub fn new(max_capacity: u64, ttl_hours: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(Duration::from_secs(ttl_hours * 3600))
            .build();

        Self { cache }
    }

    /// Get a summary from the cache
    pub fn get(&self, user_id: i64, entry_id: i64) -> Option<SummaryCacheEntry> {
        self.cache.get(&(user_id, entry_id))
    }

    /// Set a pending status for an entry
    pub fn set_pending(&self, user_id: i64, entry_id: i64) {
        self.cache
            .insert((user_id, entry_id), SummaryCacheEntry::new_pending());
    }

    /// Set a processing status for an entry
    pub fn set_processing(&self, user_id: i64, entry_id: i64) {
        self.cache
            .insert((user_id, entry_id), SummaryCacheEntry::new_processing());
    }

    /// Set a completed summary
    pub fn set_completed(&self, user_id: i64, entry_id: i64, text: String) {
        self.cache
            .insert((user_id, entry_id), SummaryCacheEntry::new_completed(text));
    }

    /// Set a failed status with error message
    pub fn set_failed(&self, user_id: i64, entry_id: i64, error: String) {
        self.cache
            .insert((user_id, entry_id), SummaryCacheEntry::new_failed(error));
    }

    /// Remove a summary from the cache
    pub fn remove(&self, user_id: i64, entry_id: i64) {
        self.cache.invalidate(&(user_id, entry_id));
    }

    /// Check if an entry has a summary (completed or in progress)
    pub fn has_summary(&self, user_id: i64, entry_id: i64) -> bool {
        self.cache.get(&(user_id, entry_id)).is_some()
    }

    /// Check if an entry has a completed summary
    pub fn has_completed_summary(&self, user_id: i64, entry_id: i64) -> bool {
        self.cache
            .get(&(user_id, entry_id))
            .map(|e| e.status == SummaryStatus::Completed)
            .unwrap_or(false)
    }

    /// List all entry IDs with summaries for a user
    pub fn list_by_user(&self, user_id: i64) -> Vec<i64> {
        self.cache
            .iter()
            .filter_map(|(key, _)| {
                if key.0 == user_id {
                    Some(key.1)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Count entries by status for a user
    pub fn count_by_status(&self, user_id: i64, status: SummaryStatus) -> usize {
        self.cache
            .iter()
            .filter(|(key, entry)| key.0 == user_id && entry.status == status)
            .count()
    }

    /// Get cache statistics
    pub fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }
}

/// Create an Arc-wrapped SummaryCache for sharing across threads
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

        // Set pending
        cache.set_pending(1, 100);
        let entry = cache.get(1, 100).unwrap();
        assert_eq!(entry.status, SummaryStatus::Pending);

        // Set processing
        cache.set_processing(1, 100);
        let entry = cache.get(1, 100).unwrap();
        assert_eq!(entry.status, SummaryStatus::Processing);

        // Set completed
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

    #[test]
    fn test_has_summary_methods() {
        let cache = SummaryCache::new(100, 24);

        assert!(!cache.has_summary(1, 100));
        assert!(!cache.has_completed_summary(1, 100));

        cache.set_pending(1, 100);
        assert!(cache.has_summary(1, 100));
        assert!(!cache.has_completed_summary(1, 100));

        cache.set_completed(1, 100, "Summary".to_string());
        assert!(cache.has_summary(1, 100));
        assert!(cache.has_completed_summary(1, 100));
    }

    #[test]
    fn test_list_by_user() {
        let cache = SummaryCache::new(100, 24);

        cache.set_completed(1, 100, "a".to_string());
        cache.set_completed(1, 101, "b".to_string());
        cache.set_completed(2, 200, "c".to_string());

        let user1_entries = cache.list_by_user(1);
        assert_eq!(user1_entries.len(), 2);
        assert!(user1_entries.contains(&100));
        assert!(user1_entries.contains(&101));

        let user2_entries = cache.list_by_user(2);
        assert_eq!(user2_entries.len(), 1);
        assert!(user2_entries.contains(&200));
    }

    #[test]
    fn test_count_by_status() {
        let cache = SummaryCache::new(100, 24);

        cache.set_pending(1, 100);
        cache.set_processing(1, 101);
        cache.set_completed(1, 102, "a".to_string());
        cache.set_completed(1, 103, "b".to_string());
        cache.set_failed(1, 104, "err".to_string());

        assert_eq!(cache.count_by_status(1, SummaryStatus::Pending), 1);
        assert_eq!(cache.count_by_status(1, SummaryStatus::Processing), 1);
        assert_eq!(cache.count_by_status(1, SummaryStatus::Completed), 2);
        assert_eq!(cache.count_by_status(1, SummaryStatus::Failed), 1);
    }
}
