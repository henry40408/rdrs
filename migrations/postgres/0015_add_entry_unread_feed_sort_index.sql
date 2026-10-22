-- Serve the reading pane's "12 / 48" count (`find_position`) from an index alone
-- on the unread list. See the SQLite migration of the same number for the
-- measurements; they were taken on SQLite only.
--
-- The count walks unread entries newer than the open one and stops at 1001.
-- That needs the sort timestamp and `feed_id` together, which neither
-- `idx_entry_unread_sort` (timestamp only) nor `idx_entry_unread_feed`
-- (`feed_id` only) has.
--
-- Partial on `read_at IS NULL`, matching `idx_entry_unread_sort` and 0011.
CREATE INDEX IF NOT EXISTS idx_entry_unread_feed_sort
    ON entry(feed_id, (COALESCE(published_at, created_at))) WHERE read_at IS NULL;
