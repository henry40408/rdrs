-- When this session last *proved* its credentials, as opposed to merely
-- presenting a cookie. Sensitive operations (passkey registration and removal)
-- require a recent value; see `middleware::auth::RecentlyAuthenticated`.
--
-- Backfilled from `created_at`, which is exactly right: a session was created
-- by a completed login, so at that instant it had just authenticated. Existing
-- sessions therefore fall outside the window immediately, which is the safe
-- direction — they are asked to re-authenticate, not waved through.
ALTER TABLE session ADD COLUMN last_authenticated_at TEXT;

UPDATE session SET last_authenticated_at = created_at WHERE last_authenticated_at IS NULL;
