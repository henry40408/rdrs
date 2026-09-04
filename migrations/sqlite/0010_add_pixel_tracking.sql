-- Which entries were actually rendered by a client, as opposed to merely
-- marked read.
--
-- `entry.read_at` is a poor "the reader looked at this" signal: marking a row
-- read from the list, or "mark all as read", sets it without the entry's HTML
-- ever being rendered. A 1x1 image appended to the rendered content fires only
-- when something rendered that content, so aggregating hits per feed answers
-- the question `read_at` cannot — "is this feed worth staying subscribed to?".
--
-- One row per (reader, entry) and no hit counter: the composite primary key is
-- the dedup key, so a client that re-requests the image, or several clients
-- that each fetch it once, still count as one open. Repeat opens and open
-- timelines are deliberately not recorded — the metric is a per-feed rate, and
-- storing more would be a tracking log of the reader's own behaviour for no
-- extra decision.
--
-- WITHOUT ROWID for the same reason as `entry_tombstone`: a narrow table whose
-- primary key is the whole row, where the extra rowid indirection buys nothing.
CREATE TABLE IF NOT EXISTS entry_open (
    user_id  INTEGER NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    entry_id INTEGER NOT NULL REFERENCES entry(id) ON DELETE CASCADE,
    first_opened_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, entry_id)
) WITHOUT ROWID;

-- `entry_id` is not a prefix of the primary key, so the `ON DELETE CASCADE`
-- from `entry` — which retention fires in batches of 500 — would otherwise scan
-- the whole table once per deleted entry. No matching index on `user_id`: that
-- one *is* the key's prefix and is already served by it.
CREATE INDEX IF NOT EXISTS idx_entry_open_entry ON entry_open(entry_id);

-- When this reader opted into pixel tracking, or NULL for opted out (the
-- default, and what every existing row is).
--
-- A timestamp rather than a boolean because it is also the baseline the open
-- rate is measured from: entries that arrived before the opt-in never carried a
-- pixel, so counting them in the denominator would report every feed as ignored
-- for as long as the pre-opt-in backlog survives. Set on the NULL -> enabled
-- transition only, so re-saving the preferences form does not move the baseline
-- forward and silently discard the data collected so far.
ALTER TABLE user_settings ADD COLUMN pixel_tracking_enabled_at TEXT;
