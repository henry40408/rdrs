-- The timezone that decides where the statistics page cuts one day from the next.
-- An IANA name; 'UTC' keeps the behaviour that shipped before it, so existing
-- rows need no backfill. Instants stay UTC everywhere else.
ALTER TABLE user_settings ADD COLUMN timezone TEXT NOT NULL DEFAULT 'UTC';
