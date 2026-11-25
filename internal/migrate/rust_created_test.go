package migrate

import (
	"context"
	"encoding/hex"
	"testing"

	"github.com/henry40408/rdrs/internal/db"
)

// rustMigration is one migration as the Rust build's sqlx recorded it.
// ExecutionTime (nanoseconds) exists only for the SQLite rows, which come from
// a real database; the PostgreSQL rows have none.
type rustMigration struct {
	Version       int64
	Description   string
	File          string
	SHA384        string
	ExecutionTime int64
}

// rustInstalledOn is the installed_on sqlx wrote for every row of the capture.
const rustInstalledOn = "2026-11-17 07:11:55"

// rustSchema holds the CREATE statement of every table and index in a database
// the Rust build had migrated to version 16, in the order and the form
// sqlite_master stored them (SQLite's own sqlite_* tables left out). The text
// is not the text of the migration files: ALTER TABLE ... ADD COLUMN edited the
// stored statement, so Run has to produce this exact result.
//
// The capture was the SQLite file internal/migrate/testdata/sqlx_migrated.sqlite3
// and testdata/vectors/migrations.json; both were replaced by the literals in
// this file. `git show 1c5b951:<path>` retrieves the originals.
var rustSchema = []string{
	`CREATE TABLE _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
)`,
	`CREATE TABLE user (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('admin', 'user')),
    disabled_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
)`,
	`CREATE TABLE category (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, name)
)`,
	`CREATE INDEX idx_category_user_id ON category(user_id)`,
	`CREATE TABLE feed (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    category_id INTEGER NOT NULL REFERENCES category(id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    title TEXT,
    description TEXT,
    site_url TEXT,
    feed_updated_at TEXT,
    fetched_at TEXT,
    fetch_error TEXT,
    etag TEXT,
    last_modified TEXT,
    custom_user_agent TEXT,
    http2_disabled INTEGER NOT NULL DEFAULT 0,
    custom_referrer TEXT,
    bucket INTEGER,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(category_id, url)
)`,
	`CREATE INDEX idx_feed_category_id ON feed(category_id)`,
	`CREATE INDEX idx_feed_bucket ON feed(bucket)`,
	`CREATE TABLE entry (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    feed_id INTEGER NOT NULL REFERENCES feed(id) ON DELETE CASCADE,
    guid TEXT NOT NULL,
    title TEXT,
    link TEXT,
    content TEXT,
    summary TEXT,
    author TEXT,
    published_at TEXT,
    read_at TEXT,
    starred_at TEXT,
    content_text TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')), full_content TEXT,
    UNIQUE(feed_id, guid)
)`,
	`CREATE INDEX idx_entry_feed_id ON entry(feed_id)`,
	`CREATE INDEX idx_entry_published_at ON entry(published_at)`,
	`CREATE INDEX idx_entry_read_at ON entry(read_at)`,
	`CREATE INDEX idx_entry_starred_at ON entry(starred_at)`,
	`CREATE INDEX idx_entry_sort_ts ON entry(COALESCE(published_at, created_at))`,
	`CREATE INDEX idx_entry_created_at ON entry(created_at)`,
	`CREATE INDEX idx_entry_starred_sort
    ON entry(COALESCE(published_at, created_at)) WHERE starred_at IS NOT NULL`,
	`CREATE INDEX idx_entry_read_sort
    ON entry(COALESCE(published_at, created_at)) WHERE read_at IS NOT NULL`,
	`CREATE INDEX idx_entry_unread_feed
    ON entry(feed_id) WHERE read_at IS NULL`,
	`CREATE INDEX idx_entry_feed_sort
    ON entry(feed_id, COALESCE(published_at, created_at))`,
	`CREATE INDEX idx_entry_unread_sort
    ON entry(COALESCE(published_at, created_at)) WHERE read_at IS NULL`,
	`CREATE TABLE entry_tombstone (
    feed_id    INTEGER NOT NULL REFERENCES feed(id) ON DELETE CASCADE,
    guid       TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (feed_id, guid)
) WITHOUT ROWID`,
	`CREATE TABLE entry_summary (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    entry_id INTEGER NOT NULL REFERENCES entry(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('pending', 'processing', 'completed', 'failed')),
    summary_text TEXT,
    error_message TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, entry_id)
)`,
	`CREATE INDEX idx_entry_summary_user_entry ON entry_summary(user_id, entry_id)`,
	`CREATE INDEX idx_entry_summary_user_status ON entry_summary(user_id, status)`,
	`CREATE INDEX idx_entry_summary_entry_id ON entry_summary(entry_id)`,
	`CREATE TABLE image (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type TEXT NOT NULL,
    entity_id INTEGER NOT NULL,
    data BLOB NOT NULL,
    content_type TEXT NOT NULL,
    source_url TEXT,
    fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(entity_type, entity_id)
)`,
	`CREATE INDEX idx_image_entity ON image(entity_type, entity_id)`,
	`CREATE TABLE user_settings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL UNIQUE REFERENCES user(id) ON DELETE CASCADE,
    entries_per_page INTEGER NOT NULL DEFAULT 30,
    save_services TEXT,
    theme TEXT,
    retention_read_days INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
, sidebar_sort TEXT NOT NULL DEFAULT 'name', sidebar_hide_read INTEGER NOT NULL DEFAULT 0, offline_keep INTEGER NOT NULL DEFAULT 0, pixel_tracking_enabled_at TEXT, timezone TEXT NOT NULL DEFAULT 'UTC')`,
	`CREATE INDEX idx_user_settings_user_id ON user_settings(user_id)`,
	`CREATE TABLE passkey (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    credential_id BLOB NOT NULL UNIQUE,
    public_key BLOB NOT NULL,
    counter INTEGER NOT NULL DEFAULT 0,
    name TEXT NOT NULL,
    transports TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_used_at TEXT
)`,
	`CREATE INDEX idx_passkey_user_id ON passkey(user_id)`,
	`CREATE INDEX idx_passkey_credential_id ON passkey(credential_id)`,
	`CREATE TABLE webauthn_challenge (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    challenge BLOB NOT NULL UNIQUE,
    user_id INTEGER REFERENCES user(id) ON DELETE CASCADE,
    challenge_type TEXT NOT NULL CHECK (challenge_type IN ('registration', 'authentication')),
    state_data TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL
)`,
	`CREATE INDEX idx_webauthn_challenge_expires_at ON webauthn_challenge(expires_at)`,
	`CREATE TABLE session (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    session_token TEXT NOT NULL UNIQUE,
    original_user_id INTEGER REFERENCES user(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    user_agent TEXT NOT NULL,
    ip_address TEXT NOT NULL,
    last_seen_at TEXT NOT NULL
, previous_token TEXT, previous_token_expires_at TEXT, last_authenticated_at TEXT)`,
	`CREATE INDEX idx_session_token ON session(session_token)`,
	`CREATE INDEX idx_session_user_id ON session(user_id)`,
	`CREATE INDEX idx_session_expires_at ON session(expires_at)`,
	`CREATE TABLE api_token (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    token TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL DEFAULT 'greader',
    label TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    user_agent TEXT NOT NULL,
    ip_address TEXT NOT NULL
)`,
	`CREATE INDEX idx_api_token_token ON api_token(token)`,
	`CREATE INDEX idx_api_token_user_id ON api_token(user_id)`,
	`CREATE INDEX idx_api_token_expires_at ON api_token(expires_at)`,
	`CREATE INDEX idx_session_previous_token ON session(previous_token)`,
	`CREATE TABLE user_invite (
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
)`,
	`CREATE UNIQUE INDEX idx_user_invite_token_hash ON user_invite(token_hash)`,
	`CREATE INDEX idx_user_invite_user_id ON user_invite(user_id)`,
	`CREATE TABLE entry_open (
    user_id  INTEGER NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    entry_id INTEGER NOT NULL REFERENCES entry(id) ON DELETE CASCADE,
    first_opened_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, entry_id)
) WITHOUT ROWID`,
	`CREATE INDEX idx_entry_open_entry ON entry_open(entry_id)`,
	`CREATE INDEX idx_entry_feed_read_sort
    ON entry(feed_id, COALESCE(published_at, created_at)) WHERE read_at IS NOT NULL`,
	`CREATE INDEX idx_entry_feed_read_at
    ON entry(feed_id, read_at) WHERE read_at IS NOT NULL`,
	`CREATE INDEX idx_entry_feed_starred_sort
    ON entry(feed_id, COALESCE(published_at, created_at)) WHERE starred_at IS NOT NULL`,
	`CREATE INDEX idx_entry_feed_created_at
    ON entry(feed_id, created_at)`,
	`CREATE INDEX idx_entry_unread_feed_sort
    ON entry(feed_id, COALESCE(published_at, created_at)) WHERE read_at IS NULL`,
}

// rustMigrationVectors maps a migrations/ directory to the version, description
// and SHA-384 of the file bytes that the Rust build's sqlx 0.9 computed for each
// of its migrations. The "sqlite" rows are also the _sqlx_migrations rows of the
// database captured in rustSchema.
var rustMigrationVectors = map[string][]rustMigration{
	"postgres": {
		{Version: 1, Description: "initial", File: "0001_initial.sql", SHA384: "e22cad0c27ef5baeb8dd5054a0c3422f41bbb364e550df387bca3d711034a9066e84d6974bb7f06638cadf49b4c535fb"},
		{Version: 2, Description: "add session metadata", File: "0002_add_session_metadata.sql", SHA384: "0238c5b4be8e96c80294e58fda81895af89de66bf972a19651b4836fc14019474e28b5afc9a5c94357e85adea948f782"},
		{Version: 3, Description: "add api token", File: "0003_add_api_token.sql", SHA384: "18456e9d355a550cec2c8294155809f697a7153a131eb3eebe16ebb0906d1e9c9aeac11c5ccc27d0c9924175ea9ba0b0"},
		{Version: 4, Description: "add session token rotation", File: "0004_add_session_token_rotation.sql", SHA384: "ed2565b9a115fcab71a1cac11ac3f5c992700a0857929b1f4e8e8966fd57a5d2fee67760de07b6e6212f011deba8ae05"},
		{Version: 5, Description: "add session last authenticated at", File: "0005_add_session_last_authenticated_at.sql", SHA384: "70051ee9205781f6af6c078e49df518101291b098fc59033b05ea8d1932b85d48e2fdfe895aa4ec2c769318bccbabc41"},
		{Version: 6, Description: "add user invite", File: "0006_add_user_invite.sql", SHA384: "587c978bdd8f08689f0a7ffd187f3e569b5b4c03257c9788c40d5bc5c091d152f6740f11b2485dc3c24e2935e380cce0"},
		{Version: 7, Description: "add sidebar preferences", File: "0007_add_sidebar_preferences.sql", SHA384: "a2874132ba50eba90e211a25d830b7c6e1f4390286a3628053c6722f267d82caa6bcf5f2d5152c64cd120711f2623054"},
		{Version: 8, Description: "add entry full content", File: "0008_add_entry_full_content.sql", SHA384: "7a7c7c244df6b8493b2c59ee4eb148d61ad7a20ce886d39357dbc6f0595b6adafeac5b59d6d4919ae50387465157ca85"},
		{Version: 9, Description: "add offline keep", File: "0009_add_offline_keep.sql", SHA384: "b989d28f2fc21c1cf72a6585f8782a2751c9af8287a7007cb51b3a4bb46165fa013ede0f4aee08ca2c66ff0807a6aab3"},
		{Version: 10, Description: "add pixel tracking", File: "0010_add_pixel_tracking.sql", SHA384: "ffc1863a2baeff7bd3b5312ba5ac79e45a4cc58a9073f66573985f0ee6f1099e3526dc393b2643e14bf22cfc93731da5"},
		{Version: 11, Description: "add entry feed read index", File: "0011_add_entry_feed_read_index.sql", SHA384: "8b6fa0923f208556f4a6f4a37774e676f30fb0881d4574e731ad52fd1fdf7c9e9b105dc335e5a6a4a298a4eb49493b58"},
		{Version: 12, Description: "add entry feed read at index", File: "0012_add_entry_feed_read_at_index.sql", SHA384: "9260ccd66bedbf8bfc3cadf8487647f11de33516dd5f6086115cd50c8d16ec806d5a9c927ca1ca8baa3b524c663ccf5d"},
		{Version: 13, Description: "add entry feed starred index", File: "0013_add_entry_feed_starred_index.sql", SHA384: "c8a962ec61388a2d74112ca51a72a118606a36efe671f295280ca726d5c29ecf64e82442f9f8d52d0f191b1f1cf72a42"},
		{Version: 14, Description: "add entry feed created at index", File: "0014_add_entry_feed_created_at_index.sql", SHA384: "bd7de03c589ca27987814b5799f0c428e2ce443e54c83d102389bb3141c016f6d00917299524dd6cb71421a421fa783f"},
		{Version: 15, Description: "add entry unread feed sort index", File: "0015_add_entry_unread_feed_sort_index.sql", SHA384: "28b4455e487fba09323459a2ed481d946957af3cdf1609a1a4abba684ba37f940a5b1881604e1dd5de44136d1fa88bea"},
		{Version: 16, Description: "add user timezone", File: "0016_add_user_timezone.sql", SHA384: "ed1b75670c856ae6f37a0455c3d399f30e1408b53614fbf1e1565d53b6ea828b2b619cf41b1be2e7f34be1c211165550"},
	},
	"sqlite": {
		{Version: 1, Description: "initial", File: "0001_initial.sql", SHA384: "b651e9761d64108da22169a15bebc03b8911f0b78007bd7d177b072bf554744040933b73393be9fd5a58f98d4666d3bd", ExecutionTime: 1802125},
		{Version: 2, Description: "add session metadata", File: "0002_add_session_metadata.sql", SHA384: "7d5f90d8baf5a250620ce9e62b088a9dd4660188f0defa47015dac861c0d668e8c0ef0d567cf422bb0cb154ea7272772", ExecutionTime: 403792},
		{Version: 3, Description: "add api token", File: "0003_add_api_token.sql", SHA384: "a670ec6f86470b0486b47d838a4b68b7240b147abd9d134d14a67b6fd4953cb39f3ee533e76f28e4a3aed70779045752", ExecutionTime: 261000},
		{Version: 4, Description: "add session token rotation", File: "0004_add_session_token_rotation.sql", SHA384: "6710a9893cb056f4c94b99a7fa24457dd2e244bc04de37d306511e0efa3de428293d48eded04f6f0af63d4917b061d48", ExecutionTime: 653917},
		{Version: 5, Description: "add session last authenticated at", File: "0005_add_session_last_authenticated_at.sql", SHA384: "181177d4c04aaf4ae967cb6b6a6f7913cbe8b9c29faa83ee15e1f224314a9c4278fe1e07df6d408f97bed805d94a62b3", ExecutionTime: 368917},
		{Version: 6, Description: "add user invite", File: "0006_add_user_invite.sql", SHA384: "43c817958862fb029eeda3170abe586349255ecfee47a1f002810e8707c423fa866fe0ad36ccb295d6f4f5f2f822a6fc", ExecutionTime: 249500},
		{Version: 7, Description: "add sidebar preferences", File: "0007_add_sidebar_preferences.sql", SHA384: "dc1439be635bcd24269a3dc66ae3dc96a6d8a7444d215c459539e2ade0f807ad2c7afba43569cbfcf9359f45e6aa34e7", ExecutionTime: 595792},
		{Version: 8, Description: "add entry full content", File: "0008_add_entry_full_content.sql", SHA384: "7a7c7c244df6b8493b2c59ee4eb148d61ad7a20ce886d39357dbc6f0595b6adafeac5b59d6d4919ae50387465157ca85", ExecutionTime: 352750},
		{Version: 9, Description: "add offline keep", File: "0009_add_offline_keep.sql", SHA384: "0c19a7cf554a98cf81015f05afb233c32757b0f9e468929529f0ab602a501e4d01353e9de7d6df3348c95513b07fa048", ExecutionTime: 333250},
		{Version: 10, Description: "add pixel tracking", File: "0010_add_pixel_tracking.sql", SHA384: "6829af18694c1f2a5c19c3670129b14eabf55623ea7ee947ad3d7fada4880a066c3b67bb357b0e1231c3a91ff2ba17b8", ExecutionTime: 440166},
		{Version: 11, Description: "add entry feed read index", File: "0011_add_entry_feed_read_index.sql", SHA384: "5cfb74fffcea73ccbb806516fe67798a993d5830618432b6d511e664281e4229f26b4ec1ec92bd15e0e2567eee97847b", ExecutionTime: 131166},
		{Version: 12, Description: "add entry feed read at index", File: "0012_add_entry_feed_read_at_index.sql", SHA384: "f777cbb938da5b808ed0236e7b4ec1f5604a6fc417105be81eb7b0d819fe094114cd0a421afc670eeeca4d4a4649a42b", ExecutionTime: 130250},
		{Version: 13, Description: "add entry feed starred index", File: "0013_add_entry_feed_starred_index.sql", SHA384: "54e246659b759027c1c09ae70a4304a24493c21d5217df7a1c15014aa7b0fe58f6c67801fce264ed6914e9007bdc8c86", ExecutionTime: 127709},
		{Version: 14, Description: "add entry feed created at index", File: "0014_add_entry_feed_created_at_index.sql", SHA384: "bd7de03c589ca27987814b5799f0c428e2ce443e54c83d102389bb3141c016f6d00917299524dd6cb71421a421fa783f", ExecutionTime: 124792},
		{Version: 15, Description: "add entry unread feed sort index", File: "0015_add_entry_unread_feed_sort_index.sql", SHA384: "1d945e41a6aec77ec33bfb48b9c8d3169fec13aeb1de233907da13379fb0fbf449ba1583b72cacf86c12cf18ea2e3ac8", ExecutionTime: 141292},
		{Version: 16, Description: "add user timezone", File: "0016_add_user_timezone.sql", SHA384: "ed1b75670c856ae6f37a0455c3d399f30e1408b53614fbf1e1565d53b6ea828b2b619cf41b1be2e7f34be1c211165550", ExecutionTime: 373541},
	},
}

// openRustMigrated rebuilds the database the Rust build left behind: the schema
// and the sqlx bookkeeping rows exactly as captured.
func openRustMigrated(t *testing.T) *db.DB {
	t.Helper()
	database := openEmpty(t)
	ctx := context.Background()
	for _, statement := range rustSchema {
		if _, err := database.ExecContext(ctx, statement); err != nil {
			t.Fatal(err)
		}
	}
	for _, row := range rustMigrationVectors["sqlite"] {
		checksum, err := hex.DecodeString(row.SHA384)
		if err != nil {
			t.Fatal(err)
		}
		_, err = database.ExecContext(ctx,
			"INSERT INTO _sqlx_migrations (version, description, installed_on, success, checksum, execution_time) VALUES ($1, $2, $3, 1, $4, $5)",
			row.Version, row.Description, rustInstalledOn, checksum, row.ExecutionTime)
		if err != nil {
			t.Fatal(err)
		}
	}
	return database
}
