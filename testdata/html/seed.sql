-- Deterministic fixture applied to a freshly migrated SQLite DB before the Rust
-- server renders the HTML goldens in internal/views/testdata/golden. The Go
-- port loads the same file, so both sides render identical state.
--
-- Placeholders replaced by the loader:
--   @SESSION_EXPIRES@  RFC 3339 instant 7 days after the capture (sessions)
--   @INVITE_HASH@      hex(HMAC(secret, "invite:" || "golden-invite-token"))

INSERT INTO user (id, username, password_hash, role, disabled_at, created_at) VALUES
  (1, 'admin', '!', 'admin', NULL, '2026-01-01 00:00:00'),
  (2, 'alice', '!', 'user', NULL, '2026-01-02 00:00:00'),
  (3, 'bob', '!', 'user', '2026-02-01 00:00:00', '2026-01-03 00:00:00');

INSERT INTO session (id, user_id, session_token, created_at, expires_at, user_agent, ip_address,
                     last_seen_at, last_authenticated_at) VALUES
  (1, 1, 'golden-admin-session', '2026-11-17T00:00:00.000000+00:00', '@SESSION_EXPIRES@',
   'Mozilla/5.0 (X11; Linux x86_64) Firefox/130.0', '203.0.113.7',
   '2026-11-17T00:00:00.000000+00:00', '2026-11-17T00:00:00.000000+00:00'),
  (2, 2, 'golden-alice-session', '2026-11-17T00:00:00.000000+00:00', '@SESSION_EXPIRES@',
   'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0) Safari/604.1', '203.0.113.8',
   '2026-11-17T00:00:00.000000+00:00', '2026-11-17T00:00:00.000000+00:00');

INSERT INTO user_settings (user_id, entries_per_page, theme, retention_read_days, sidebar_sort,
                           sidebar_hide_read, offline_keep, pixel_tracking_enabled_at, timezone) VALUES
  (1, 30, NULL, 0, 'name', 0, 0, NULL, 'UTC'),
  (2, 10, 'dark', 30, 'unread', 1, 5, '2026-11-01T00:00:00+00:00', 'Asia/Taipei');

INSERT INTO category (id, user_id, name, created_at) VALUES
  (1, 1, 'Tech', '2026-01-05 00:00:00'),
  (2, 1, 'News', '2026-01-06 00:00:00'),
  (3, 1, 'Empty', '2026-01-07 00:00:00'),
  (4, 2, 'Alice reads', '2026-01-08 00:00:00');

-- bucket stays NULL so the background sync never touches these hosts.
INSERT INTO feed (id, category_id, url, title, description, site_url, feed_updated_at, fetched_at,
                  fetch_error, custom_user_agent, http2_disabled, custom_referrer, bucket,
                  created_at, updated_at) VALUES
  (1, 1, 'https://alpha.example/feed.xml', 'Alpha Blog', 'Posts about alpha', 'https://alpha.example/',
   '2026-11-16T12:00:00+00:00', '2026-11-16T12:30:00+00:00', NULL, NULL, 0, NULL, NULL,
   '2026-01-10 00:00:00', '2026-11-16 12:30:00'),
  (2, 2, 'https://beta.example/rss', 'Beta News', NULL, 'https://beta.example/',
   '2026-11-15T12:00:00+00:00', '2026-11-16T12:30:00+00:00', NULL, 'GoldenBot/1.0', 1,
   'https://ref.example/', NULL, '2026-01-11 00:00:00', '2026-11-16 12:30:00'),
  (3, 1, 'https://broken.example/feed', NULL, NULL, NULL, NULL, '2026-11-16T12:30:00+00:00',
   'HTTP 404 Not Found', NULL, 0, NULL, NULL, '2026-01-12 00:00:00', '2026-11-16 12:30:00'),
  (4, 4, 'https://alice.example/atom', 'Alice Feed', NULL, 'https://alice.example/', NULL, NULL,
   NULL, NULL, 0, NULL, NULL, '2026-01-13 00:00:00', '2026-01-13 00:00:00');

INSERT INTO entry (id, feed_id, guid, title, link, content, summary, author, published_at, read_at,
                   starred_at, content_text, full_content, created_at, updated_at) VALUES
  (1, 1, 'a-1', 'Alpha one', 'https://alpha.example/1',
   '<p>First <b>alpha</b> post with a <a href="https://alpha.example/x?utm_source=feed">link</a>.</p>',
   'Alpha summary one', 'Alice Author', '2026-11-16 10:00:00', NULL, NULL,
   'First alpha post with a link.', NULL, '2026-11-16 10:05:00', '2026-11-16 10:05:00'),
  (2, 1, 'a-2', 'Alpha two', 'https://alpha.example/2',
   '<p>Second post</p><img src="https://alpha.example/img.png" alt="pic">',
   NULL, NULL, '2026-11-15 09:00:00', '2026-11-16 08:00:00', NULL,
   'Second post', NULL, '2026-11-15 09:05:00', '2026-11-15 09:05:00'),
  (3, 1, 'a-3', 'Alpha three <starred> & escaped', 'https://alpha.example/3',
   '<p>Third &amp; starred</p>', 'Third summary', 'Bob', '2026-11-14 09:00:00', NULL,
   '2026-11-16 09:00:00', 'Third & starred', NULL, '2026-11-14 09:05:00', '2026-11-14 09:05:00'),
  (4, 1, 'a-4', 'Alpha four (full content)', 'https://alpha.example/4',
   '<p>Teaser only</p>', NULL, NULL, '2026-11-13 09:00:00', NULL, NULL,
   'Teaser only', '<p>Full article body</p><p>Second paragraph</p>', '2026-11-13 09:05:00',
   '2026-11-13 09:05:00'),
  (5, 2, 'b-1', 'Beta headline', 'https://beta.example/1',
   '<p>Beta news body</p>', 'Beta summary', NULL, '2026-11-16 11:00:00', NULL, NULL,
   'Beta news body', NULL, '2026-11-16 11:05:00', '2026-11-16 11:05:00'),
  (6, 2, 'b-2', 'Beta older', NULL, '<p>No link here</p>', NULL, NULL, '2026-11-10 11:00:00',
   '2026-11-11 00:00:00', '2026-11-12 00:00:00', 'No link here', NULL, '2026-11-10 11:05:00',
   '2026-11-10 11:05:00'),
  (7, 2, 'b-3', NULL, 'https://beta.example/3', '<p>Untitled entry</p>', NULL, NULL, NULL, NULL,
   NULL, 'Untitled entry', NULL, '2026-11-09 11:05:00', '2026-11-09 11:05:00'),
  (8, 4, 'x-1', 'Alice entry', 'https://alice.example/1', '<p>For alice</p>', NULL, NULL,
   '2026-11-16 07:00:00', NULL, NULL, 'For alice', NULL, '2026-11-16 07:05:00', '2026-11-16 07:05:00');

INSERT INTO entry_summary (user_id, entry_id, status, summary_text, error_message, created_at,
                           updated_at) VALUES
  (1, 1, 'completed', 'A **short** summary of alpha one.', NULL, '2026-11-16 11:00:00', '2026-11-16 11:00:00'),
  (1, 5, 'failed', NULL, 'Kagi API error: rate limited', '2026-11-16 11:10:00', '2026-11-16 11:10:00');

INSERT INTO entry_open (user_id, entry_id, first_opened_at) VALUES (2, 8, '2026-11-16 08:00:00');

INSERT INTO user_invite (user_id, token_hash, created_at, expires_at, consumed_at) VALUES
  (3, '@INVITE_HASH@', '2026-11-17T00:00:00.000000+00:00', '@SESSION_EXPIRES@', NULL);

INSERT INTO api_token (user_id, token, kind, label, created_at, expires_at, last_seen_at,
                       user_agent, ip_address) VALUES
  (1, 'rdrs_gr_golden-api-token', 'greader', 'Reader app', '2026-11-10T00:00:00+00:00',
   '@SESSION_EXPIRES@', '2026-11-16T00:00:00+00:00', 'FeedMe/1.0', '198.51.100.4');

INSERT INTO passkey (user_id, credential_id, public_key, counter, name, transports, created_at,
                     last_used_at) VALUES
  (1, X'0102030405', X'7B7D', 3, 'Laptop key', NULL, '2026-10-01T00:00:00+00:00', '2026-11-16T00:00:00+00:00');
