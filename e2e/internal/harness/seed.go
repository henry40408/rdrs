package harness

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"net/url"

	// The pure-Go driver, the one the server itself uses.
	_ "modernc.org/sqlite"
)

// NewEntry is one entry to insert.
type NewEntry struct {
	FeedID  int64
	GUID    string
	Title   string
	Link    string
	Content string
	// Summary is NULL when empty.
	Summary string
	// PublishedOffset is a SQLite modifier applied to datetime('now', ?), such
	// as "-2 hours". Empty means now.
	PublishedOffset string
}

// Seed writes test data straight into the server's SQLite file, skipping the
// upstream fetch and sync a UI-driven Given would need. These writes never bust
// the sidebar cache, hence RDRS_DISABLE_SIDEBAR_CACHE in the server's env.
type Seed struct {
	db *sql.DB
}

// OpenSeed opens the database of a started server.
func OpenSeed(dbPath string) (*Seed, error) {
	// mode=rw: a wrong path must fail, not seed an empty second file. The busy
	// timeout lets the server and the seed share the writer.
	dsn := "file:" + url.PathEscape(dbPath) + "?mode=rw&_pragma=busy_timeout(10000)"
	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, err
	}
	db.SetMaxOpenConns(4)
	if err := db.Ping(); err != nil {
		_ = db.Close()
		return nil, fmt.Errorf("opening %s: %w", dbPath, err)
	}
	return &Seed{db: db}, nil
}

// Close releases the connection pool.
func (s *Seed) Close() {
	_ = s.db.Close()
}

// InsertEntries inserts entries and returns their ids, in the order given.
func (s *Seed) InsertEntries(entries []NewEntry) ([]int64, error) {
	tx, err := s.db.Begin()
	if err != nil {
		return nil, err
	}
	defer func() { _ = tx.Rollback() }()
	ids := make([]int64, 0, len(entries))
	for _, entry := range entries {
		id, err := insertEntry(tx, entry)
		if err != nil {
			return nil, err
		}
		ids = append(ids, id)
	}
	return ids, tx.Commit()
}

func insertEntry(tx *sql.Tx, entry NewEntry) (int64, error) {
	offset := entry.PublishedOffset
	if offset == "" {
		offset = "0 seconds"
	}
	var summary any
	if entry.Summary != "" {
		summary = entry.Summary
	}
	_, err := tx.Exec(`INSERT OR IGNORE INTO entry (feed_id, guid, title, link, content, summary, published_at)
		VALUES (?, ?, ?, ?, ?, ?, datetime('now', ?))`,
		entry.FeedID, entry.GUID, entry.Title, entry.Link, entry.Content, summary, offset)
	if err != nil {
		return 0, err
	}
	var id int64
	err = tx.QueryRow("SELECT id FROM entry WHERE feed_id = ? AND guid = ?", entry.FeedID, entry.GUID).Scan(&id)
	if err != nil {
		return 0, fmt.Errorf("no entry with guid `%s` after insert: %w", entry.GUID, err)
	}
	return id, nil
}

// SeedTestEntries inserts count numbered entries into a feed, newest first.
func (s *Seed) SeedTestEntries(feedID int64, count int) ([]int64, error) {
	entries := make([]NewEntry, 0, count)
	for i := 1; i <= count; i++ {
		entries = append(entries, NewEntry{
			FeedID:  feedID,
			GUID:    fmt.Sprintf("test-guid-%d-%d", feedID, i),
			Title:   fmt.Sprintf("Test Entry %d", i),
			Link:    fmt.Sprintf("https://example.com/entry/%d", i),
			Content: fmt.Sprintf("<p>Content for test entry %d</p>", i),
			Summary: fmt.Sprintf("Summary for entry %d", i),
			// Seconds, not hours: an hour-sized gap puts entries on the previous
			// UTC day during the first hours after midnight, which empties
			// /entries/today.
			PublishedOffset: fmt.Sprintf("-%d seconds", i),
		})
	}
	return s.InsertEntries(entries)
}

func (s *Seed) scalar(what, query string, args ...any) (int64, error) {
	var id int64
	if err := s.db.QueryRow(query, args...).Scan(&id); err != nil {
		return 0, fmt.Errorf("%s: %w", what, err)
	}
	return id, nil
}

func (s *Seed) exec(query string, args ...any) error {
	_, err := s.db.Exec(query, args...)
	return err
}

// UserID is the row id of a user.
func (s *Seed) UserID(username string) (int64, error) {
	return s.scalar("user `"+username+"` not found", "SELECT id FROM user WHERE username = ?", username)
}

// CreateCategory creates a category if it is not there, returning its id
// either way.
func (s *Seed) CreateCategory(userID int64, name string) (int64, error) {
	if err := s.exec("INSERT OR IGNORE INTO category (user_id, name) VALUES (?, ?)", userID, name); err != nil {
		return 0, err
	}
	return s.CategoryID(userID, name)
}

// DeleteCategory removes a category by name.
func (s *Seed) DeleteCategory(userID int64, name string) error {
	return s.exec("DELETE FROM category WHERE user_id = ? AND name = ?", userID, name)
}

// CategoryID is the id of a category.
func (s *Seed) CategoryID(userID int64, name string) (int64, error) {
	return s.scalar("category `"+name+"` not found", "SELECT id FROM category WHERE user_id = ? AND name = ?", userID, name)
}

// CreateFeed creates a feed if its URL is not already taken, returning its id.
// An empty title means the URL.
func (s *Seed) CreateFeed(categoryID int64, feedURL, title string) (int64, error) {
	if title == "" {
		title = feedURL
	}
	if err := s.exec("INSERT OR IGNORE INTO feed (category_id, url, title) VALUES (?, ?, ?)", categoryID, feedURL, title); err != nil {
		return 0, err
	}
	return s.scalar("feed `"+feedURL+"` not found after insert", "SELECT id FROM feed WHERE url = ?", feedURL)
}

// InsertIcon stores a feed's favicon. An empty sourceURL stores NULL.
func (s *Seed) InsertIcon(feedID int64, data []byte, contentType, sourceURL string) error {
	var source any
	if sourceURL != "" {
		source = sourceURL
	}
	return s.exec(`INSERT OR REPLACE INTO image (entity_type, entity_id, data, content_type, source_url)
		VALUES ('feed', ?, ?, ?, ?)`, feedID, data, contentType, source)
}

// MarkRead sets read_at relative to now, such as "-2 hours".
func (s *Seed) MarkRead(entryID int64, relativeTime string) error {
	return s.exec("UPDATE entry SET read_at = datetime('now', ?) WHERE id = ?", relativeTime, entryID)
}

// MarkStarred sets starred_at relative to now.
func (s *Seed) MarkStarred(entryID int64, relativeTime string) error {
	return s.exec("UPDATE entry SET starred_at = datetime('now', ?) WHERE id = ?", relativeTime, entryID)
}

// InsertSummary stores a completed summary.
func (s *Seed) InsertSummary(entryID, userID int64, text string) error {
	return s.exec(`INSERT OR IGNORE INTO entry_summary (user_id, entry_id, status, summary_text)
		VALUES (?, ?, 'completed', ?)`, userID, entryID, text)
}

// InsertFailedSummary stores a failed summary.
func (s *Seed) InsertFailedSummary(entryID, userID int64, errorMessage string) error {
	return s.exec(`INSERT OR IGNORE INTO entry_summary (user_id, entry_id, status, error_message)
		VALUES (?, ?, 'failed', ?)`, userID, entryID, errorMessage)
}

// InsertPendingSummary stores a pending summary.
func (s *Seed) InsertPendingSummary(entryID, userID int64) error {
	return s.exec("INSERT OR IGNORE INTO entry_summary (user_id, entry_id, status) VALUES (?, ?, 'pending')", userID, entryID)
}

// SetEntryContent replaces an entry's content.
func (s *Seed) SetEntryContent(entryID int64, html string) error {
	return s.exec("UPDATE entry SET content = ? WHERE id = ?", html, entryID)
}

// SetEntryLink replaces an entry's link.
func (s *Seed) SetEntryLink(entryID int64, link string) error {
	return s.exec("UPDATE entry SET link = ? WHERE id = ?", link, entryID)
}

// ConfigureKagi seeds a fake Kagi config so the Summarize button renders; the
// mock upstream answers.
func (s *Seed) ConfigureKagi(userID int64, sessionToken string) error {
	payload, err := json.Marshal(map[string]any{"kagi": map[string]string{"session_token": sessionToken}})
	if err != nil {
		return err
	}
	var existing int64
	err = s.db.QueryRow("SELECT id FROM user_settings WHERE user_id = ?", userID).Scan(&existing)
	if err == sql.ErrNoRows {
		return s.exec("INSERT INTO user_settings (user_id, save_services) VALUES (?, ?)", userID, string(payload))
	}
	if err != nil {
		return err
	}
	return s.exec("UPDATE user_settings SET save_services = ? WHERE user_id = ?", string(payload), userID)
}

// EnablePixelTracking opts a reader into open tracking; entries seeded before
// this carry no pixel.
func (s *Seed) EnablePixelTracking(userID int64) error {
	return s.exec(`INSERT INTO user_settings (user_id, pixel_tracking_enabled_at)
		VALUES (?, datetime('now'))
		ON CONFLICT(user_id) DO UPDATE SET pixel_tracking_enabled_at = datetime('now')`, userID)
}

// RecordOpensForFeed records every entry of a feed as opened, as an external
// client fetching pixels would; a browser-driven scenario cannot reach it.
func (s *Seed) RecordOpensForFeed(userID, feedID int64) error {
	return s.exec(`INSERT INTO entry_open (user_id, entry_id)
		SELECT ?, id FROM entry WHERE feed_id = ?
		ON CONFLICT (user_id, entry_id) DO NOTHING`, userID, feedID)
}

// MakeAdmin promotes a user.
func (s *Seed) MakeAdmin(userID int64) error {
	return s.exec("UPDATE user SET role = 'admin' WHERE id = ?", userID)
}

// EntryIDByTitle is the id of a user's entry.
func (s *Seed) EntryIDByTitle(userID int64, title string) (int64, error) {
	return s.scalar("entry `"+title+"` not found", `SELECT e.id FROM entry e
		JOIN feed f ON e.feed_id = f.id
		JOIN category c ON f.category_id = c.id
		WHERE c.user_id = ? AND e.title = ?`, userID, title)
}

// FeedIDByTitle is the id of a user's feed.
func (s *Seed) FeedIDByTitle(userID int64, feedTitle string) (int64, error) {
	return s.scalar("feed `"+feedTitle+"` not found", `SELECT f.id FROM feed f
		JOIN category c ON f.category_id = c.id
		WHERE c.user_id = ? AND f.title = ?`, userID, feedTitle)
}

// FirstFeedID is the id of a user's first feed.
func (s *Seed) FirstFeedID(userID int64) (int64, error) {
	return s.scalar("no feed found for user", `SELECT f.id FROM feed f
		JOIN category c ON f.category_id = c.id
		WHERE c.user_id = ? LIMIT 1`, userID)
}

// MarkCategoryRead marks every entry of a category read.
func (s *Seed) MarkCategoryRead(userID int64, categoryName string) error {
	return s.exec(`UPDATE entry SET read_at = datetime('now')
		WHERE feed_id IN (
			SELECT f.id FROM feed f
			JOIN category c ON f.category_id = c.id
			WHERE c.user_id = ? AND c.name = ?
		)`, userID, categoryName)
}

// MarkFeedRead marks every entry of a feed read.
func (s *Seed) MarkFeedRead(userID int64, feedTitle string) error {
	return s.exec(`UPDATE entry SET read_at = datetime('now')
		WHERE feed_id IN (
			SELECT f.id FROM feed f
			JOIN category c ON f.category_id = c.id
			WHERE c.user_id = ? AND f.title = ?
		)`, userID, feedTitle)
}

// EntryTitles lists every entry a user can see, newest first.
func (s *Seed) EntryTitles(userID int64) ([]string, error) {
	rows, err := s.db.Query(`SELECT e.title FROM entry e
		JOIN feed f ON e.feed_id = f.id
		JOIN category c ON f.category_id = c.id
		WHERE c.user_id = ?
		ORDER BY e.published_at DESC`, userID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var titles []string
	for rows.Next() {
		var title string
		if err := rows.Scan(&title); err != nil {
			return nil, err
		}
		titles = append(titles, title)
	}
	return titles, rows.Err()
}
