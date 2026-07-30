-- Periodic session-token rotation (OWASP Session Management Cheat Sheet,
-- "Renewal Timeout") needs the token it replaces to stay valid for a short
-- safety interval: requests already in flight when the rotation lands are
-- still carrying the old cookie, and rejecting them would sign out an active
-- browser at random.
--
-- Both columns are nullable and unset for a session that has never rotated.
-- Additive, so no session is invalidated by this migration.
ALTER TABLE session ADD COLUMN previous_token TEXT;
ALTER TABLE session ADD COLUMN previous_token_expires_at TEXT;

CREATE INDEX IF NOT EXISTS idx_session_previous_token ON session(previous_token);
