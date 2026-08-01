-- Accounts are created by an admin, who then hands the new user a one-time
-- link to set their own password. Self-service registration is gone: an
-- anonymous endpoint that accepts a username inevitably answers whether that
-- username exists, and with no email channel there is no way to make that
-- answer ambiguous (the caller can simply try to sign in with the password
-- they just submitted). Removing the endpoint removes the question.
--
-- The same row also backs an admin-issued password reset for an account that
-- already has one; the two differ only in whether the target's current
-- password still works while the invite is outstanding (it does).
CREATE TABLE IF NOT EXISTS user_invite (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    -- The token is stored as an HMAC under RDRS_SECRET (secret::DOMAIN_INVITE),
    -- never in the clear: this string is worth an account takeover, and a
    -- database copy on its own must not be enough to mint a working link.
    token_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    -- Set once, atomically, by the redemption that spends it. NULL means live.
    consumed_at TEXT
);

-- Redemption looks the invite up by hashed token; the admin page lists them
-- per user. Both are indexed.
CREATE UNIQUE INDEX IF NOT EXISTS idx_user_invite_token_hash ON user_invite(token_hash);
CREATE INDEX IF NOT EXISTS idx_user_invite_user_id ON user_invite(user_id);
