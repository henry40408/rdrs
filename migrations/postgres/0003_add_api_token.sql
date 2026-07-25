-- GReader ClientLogin credentials live apart from web sessions: their own row,
-- their own expiry, independently revocable.
CREATE TABLE IF NOT EXISTS api_token (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES "user"(id) ON DELETE CASCADE,
    token TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL DEFAULT 'greader',
    label TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ NOT NULL,
    user_agent TEXT NOT NULL,
    ip_address TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_api_token_token ON api_token(token);
CREATE INDEX IF NOT EXISTS idx_api_token_user_id ON api_token(user_id);
CREATE INDEX IF NOT EXISTS idx_api_token_expires_at ON api_token(expires_at);
