-- Sidebar display preferences.
--
-- `sidebar_sort` decides the order of the category list (and of the open
-- category's feed list): 'name' keeps the server's A-Z ordering, 'unread'
-- puts the busiest groups first. Stored as text rather than an integer so a
-- third ordering doesn't need a migration to stay readable in the database.
--
-- `sidebar_hide_read` drops fully-read categories and feeds from the list.
-- Both default to the behaviour that shipped before them, so existing rows
-- need no backfill.
ALTER TABLE user_settings ADD COLUMN sidebar_sort TEXT NOT NULL DEFAULT 'name';
ALTER TABLE user_settings ADD COLUMN sidebar_hide_read INTEGER NOT NULL DEFAULT 0;
