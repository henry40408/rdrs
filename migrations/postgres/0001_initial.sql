-- Consolidated PostgreSQL schema, translated from the SQLite baseline (rdrs
-- user_version 10). Type mapping: INTEGER PK AUTOINCREMENT -> BIGINT GENERATED
-- ALWAYS AS IDENTITY; id/FK columns -> BIGINT; timestamps -> TIMESTAMPTZ; BLOB
-- -> BYTEA; 0/1 flags -> BOOLEAN; WITHOUT ROWID dropped (no PG equivalent);
-- expression indexes use PG's double-paren form. The app binds timestamps; the
-- now() DEFAULTs remain only as a fallback.

CREATE TABLE IF NOT EXISTS "user" (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('admin', 'user')),
    disabled_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS session (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES "user"(id) ON DELETE CASCADE,
    session_token TEXT NOT NULL UNIQUE,
    original_user_id BIGINT REFERENCES "user"(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_session_token ON session(session_token);
CREATE INDEX IF NOT EXISTS idx_session_user_id ON session(user_id);
CREATE INDEX IF NOT EXISTS idx_session_expires_at ON session(expires_at);

CREATE TABLE IF NOT EXISTS category (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES "user"(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(user_id, name)
);
CREATE INDEX IF NOT EXISTS idx_category_user_id ON category(user_id);

CREATE TABLE IF NOT EXISTS feed (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    category_id BIGINT NOT NULL REFERENCES category(id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    title TEXT,
    description TEXT,
    site_url TEXT,
    feed_updated_at TIMESTAMPTZ,
    fetched_at TIMESTAMPTZ,
    fetch_error TEXT,
    etag TEXT,
    last_modified TEXT,
    custom_user_agent TEXT,
    http2_disabled BOOLEAN NOT NULL DEFAULT FALSE,
    custom_referrer TEXT,
    bucket INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(category_id, url)
);
CREATE INDEX IF NOT EXISTS idx_feed_category_id ON feed(category_id);
CREATE INDEX IF NOT EXISTS idx_feed_bucket ON feed(bucket);

CREATE TABLE IF NOT EXISTS entry (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    feed_id BIGINT NOT NULL REFERENCES feed(id) ON DELETE CASCADE,
    guid TEXT NOT NULL,
    title TEXT,
    link TEXT,
    content TEXT,
    summary TEXT,
    author TEXT,
    published_at TIMESTAMPTZ,
    read_at TIMESTAMPTZ,
    starred_at TIMESTAMPTZ,
    content_text TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(feed_id, guid)
);
CREATE INDEX IF NOT EXISTS idx_entry_feed_id ON entry(feed_id);
CREATE INDEX IF NOT EXISTS idx_entry_published_at ON entry(published_at);
CREATE INDEX IF NOT EXISTS idx_entry_read_at ON entry(read_at);
CREATE INDEX IF NOT EXISTS idx_entry_starred_at ON entry(starred_at);
CREATE INDEX IF NOT EXISTS idx_entry_sort_ts ON entry((COALESCE(published_at, created_at)));
CREATE INDEX IF NOT EXISTS idx_entry_created_at ON entry(created_at);
CREATE INDEX IF NOT EXISTS idx_entry_starred_sort
    ON entry((COALESCE(published_at, created_at))) WHERE starred_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_entry_read_sort
    ON entry((COALESCE(published_at, created_at))) WHERE read_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_entry_unread_feed
    ON entry(feed_id) WHERE read_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_entry_feed_sort
    ON entry(feed_id, (COALESCE(published_at, created_at)));
CREATE INDEX IF NOT EXISTS idx_entry_unread_sort
    ON entry((COALESCE(published_at, created_at))) WHERE read_at IS NULL;

CREATE TABLE IF NOT EXISTS entry_tombstone (
    feed_id    BIGINT NOT NULL REFERENCES feed(id) ON DELETE CASCADE,
    guid       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (feed_id, guid)
);

CREATE TABLE IF NOT EXISTS entry_summary (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES "user"(id) ON DELETE CASCADE,
    entry_id BIGINT NOT NULL REFERENCES entry(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('pending', 'processing', 'completed', 'failed')),
    summary_text TEXT,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(user_id, entry_id)
);
CREATE INDEX IF NOT EXISTS idx_entry_summary_user_entry ON entry_summary(user_id, entry_id);
CREATE INDEX IF NOT EXISTS idx_entry_summary_user_status ON entry_summary(user_id, status);
CREATE INDEX IF NOT EXISTS idx_entry_summary_entry_id ON entry_summary(entry_id);

CREATE TABLE IF NOT EXISTS image (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    entity_type TEXT NOT NULL,
    entity_id BIGINT NOT NULL,
    data BYTEA NOT NULL,
    content_type TEXT NOT NULL,
    source_url TEXT,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(entity_type, entity_id)
);
CREATE INDEX IF NOT EXISTS idx_image_entity ON image(entity_type, entity_id);

CREATE TABLE IF NOT EXISTS user_settings (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT NOT NULL UNIQUE REFERENCES "user"(id) ON DELETE CASCADE,
    entries_per_page INTEGER NOT NULL DEFAULT 30,
    save_services TEXT,
    theme TEXT,
    retention_read_days INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_user_settings_user_id ON user_settings(user_id);

CREATE TABLE IF NOT EXISTS passkey (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES "user"(id) ON DELETE CASCADE,
    credential_id BYTEA NOT NULL UNIQUE,
    public_key BYTEA NOT NULL,
    counter BIGINT NOT NULL DEFAULT 0,
    name TEXT NOT NULL,
    transports TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_passkey_user_id ON passkey(user_id);
CREATE INDEX IF NOT EXISTS idx_passkey_credential_id ON passkey(credential_id);

CREATE TABLE IF NOT EXISTS webauthn_challenge (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    challenge BYTEA NOT NULL UNIQUE,
    user_id BIGINT REFERENCES "user"(id) ON DELETE CASCADE,
    challenge_type TEXT NOT NULL CHECK (challenge_type IN ('registration', 'authentication')),
    state_data TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_webauthn_challenge_expires_at ON webauthn_challenge(expires_at);
