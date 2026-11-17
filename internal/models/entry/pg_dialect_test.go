package entry_test

import (
	"context"
	"os"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/search"
	"github.com/henry40408/rdrs/internal/timezone"
)

// Port of tests/postgres_test.rs. It drives the SQL paths that fork between
// SQLite and PostgreSQL against a real server, so it is skipped without
// TEST_DATABASE_URL. Everything stays in one test because the sections share
// one database and one seeded account, exactly like the Rust original.

func utc(t *testing.T, text string) time.Time {
	t.Helper()
	parsed, err := time.Parse(time.RFC3339, text)
	must(t, err)
	return parsed.UTC()
}

// pgSmoke is the live database with the seeded "pguser" account and feed.
type pgSmoke struct {
	t        *testing.T
	database *db.DB
	userID   int64
	feedID   int64
}

func (s *pgSmoke) upsert(guid string, title *string, publishedAt *time.Time) entry.UpsertResult {
	s.t.Helper()
	result, err := entry.UpsertEntry(context.Background(), s.database, &entry.Upsert{
		FeedID: s.feedID, GUID: guid, Title: title, Link: ptr("l"), PublishedAt: publishedAt,
	})
	must(s.t, err)
	return result
}

func (s *pgSmoke) exec(query string, args ...any) {
	s.t.Helper()
	_, err := s.database.ExecContext(context.Background(), query, args...)
	must(s.t, err)
}

func (s *pgSmoke) list(filter *entry.Filter, params *entry.ContinuationParams) []entry.WithFeed {
	s.t.Helper()
	rows, err := entry.ListByUserWithContinuation(context.Background(), s.database, s.userID, filter, params)
	must(s.t, err)
	return rows
}

func (s *pgSmoke) titles(filter *entry.Filter) map[string]bool {
	s.t.Helper()
	rows, err := entry.ListByUser(context.Background(), s.database, entry.ListQuery{
		UserID: s.userID, Filter: filter, SortOrder: entry.SortPublishedAt, Limit: 50,
	})
	must(s.t, err)
	found := map[string]bool{}
	for _, row := range rows {
		if row.Title != nil {
			found[*row.Title] = true
		}
	}
	return found
}

func TestPgDialectSmoke(t *testing.T) {
	url := os.Getenv("TEST_DATABASE_URL")
	if url == "" {
		t.Skip("TEST_DATABASE_URL unset; skipping the PostgreSQL integration test")
	}
	ctx := context.Background()
	database := open(t, url, config.Postgres)
	user, err := models.CreateUser(ctx, database, "pguser", "hash", models.RoleUser)
	must(t, err)
	category, err := models.CreateCategory(ctx, database, user.ID, "Tech")
	must(t, err)
	feed, err := models.CreateFeed(ctx, database, models.CreateFeedParams{CategoryID: category.ID, URL: "https://example.com/feed.xml", Title: ptr("Feed")})
	must(t, err)
	s := &pgSmoke{t: t, database: database, userID: user.ID, feedID: feed.ID}

	ids, last := s.checkCursorPagination()
	s.checkStatistics()
	s.checkNeighbors(ids[2])
	s.checkReadSnapshot(last)
	s.checkMarkAllReadBounds(last)
	s.checkTimestampBinds()
	s.checkRetentionPrune(last)
	s.checkOfflineSet()
	s.checkDailyReadCounts(ids, last.ID)
	s.checkAdminStats()
	s.checkUnreadCounts(category.ID)
	s.checkUniqueViolationMapping()
	s.checkSearchQueryLanguage()
	s.checkExpiredSessionSweep()
	s.checkAPITokens()
	s.checkOpenTracking()
	s.checkAtomicCredentialRevocation()
	s.checkConcurrentSetupAdmitsOne()
	s.checkInviteReissueRollsBack()
}

// composite-cursor pagination (to_char cursor + %Y-%m-%d %H:%M:%S) and the
// guarded upsert UPDATE (IS DISTINCT FROM fork).
func (s *pgSmoke) checkCursorPagination() (ids []int64, last entry.WithFeed) {
	t, ctx := s.t, context.Background()
	noon, yesterday := utc(t, "2026-07-07T12:00:00Z"), utc(t, "2026-07-06T09:30:00Z")
	for _, seed := range []struct {
		guid string
		at   time.Time
	}{{"g1", noon}, {"g2", noon}, {"g3", yesterday}} { // g1 and g2 share a second: id tie-break
		ids = append(ids, s.upsert(seed.guid, ptr("t"), &seed.at).ID)
	}

	// The no-op guard is the one predicate that differs per dialect: identical
	// values must not rewrite the row, a changed column must.
	if got := s.upsert("g1", ptr("t"), &noon); got.Outcome != entry.Unchanged || got.ID != ids[0] {
		t.Errorf("identical re-upsert must be Unchanged on PG, got %+v", got)
	}
	if got := s.upsert("g1", ptr("t changed"), &noon); got.Outcome != entry.Updated || got.ID != ids[0] {
		t.Errorf("changed column must be Updated on PG, got %+v", got)
	}
	s.upsert("g1", ptr("t"), &noon) // restore the title for the pagination assertions

	filter := &entry.Filter{}
	firstPage := &entry.ContinuationParams{Limit: 1, SortOrder: entry.SortPublishedAt}
	page1 := s.list(filter, firstPage)
	if len(page1) != 1 {
		t.Fatalf("page 1 = %d entries", len(page1))
	}
	last = page1[0]
	sortTS, err := entry.FetchSortTS(ctx, s.database, last.ID, entry.SortPublishedAt)
	must(t, err)
	// The cursor string must be the SQLite-compatible space format, not RFC3339.
	if sortTS == nil || *sortTS != "2026-07-07 12:00:00" {
		t.Fatalf("cursor ts = %v, want %%Y-%%m-%%d %%H:%%M:%%S", sortTS)
	}
	// The in-query sort_ts (to_char on PG) must match it exactly.
	listed, err := entry.ListByUserWithContinuationTS(ctx, s.database, s.userID, filter, firstPage)
	must(t, err)
	if listed[0].SortTS == nil || *listed[0].SortTS != *sortTS {
		t.Errorf("in-query sort_ts = %v, want %s", listed[0].SortTS, *sortTS)
	}

	page2 := s.list(filter, &entry.ContinuationParams{
		Limit: 10, SortOrder: entry.SortPublishedAt,
		Continuation: entry.CompositeCursor{SortTS: *sortTS, ID: last.ID},
	})
	if len(page2) != 2 {
		t.Fatalf("cursor must return the two remaining entries, got %d", len(page2))
	}
	if !(page2[0].ID < last.ID || page2[0].PublishedAt.Before(*last.PublishedAt)) {
		t.Error("continuation must not repeat or precede the cursor row")
	}
	return ids, last
}

// personal overview and category/feed stats (date range vs timestamp).
func (s *pgSmoke) checkStatistics() {
	t, ctx := s.t, context.Background()
	overview, err := models.GetPersonalOverview(ctx, s.database, s.userID, "2026-07-01", "2026-07-31")
	must(t, err)
	if overview.TotalEntries != 3 {
		t.Errorf("total entries = %d, all 3 fall in range", overview.TotalEntries)
	}
	byCategory, err := models.GetEntriesByCategory(ctx, s.database, s.userID, "2026-07-01", "2026-07-31")
	must(t, err)
	if len(byCategory) != 1 || byCategory[0].Count != 3 {
		t.Errorf("by category = %+v", byCategory)
	}
	topFeeds, err := models.GetTopFeeds(ctx, s.database, models.TopFeedsQuery{UserID: s.userID, From: "2026-07-01", To: "2026-07-31", Limit: 10})
	must(t, err)
	if len(topFeeds) != 1 || topFeeds[0].Count != 3 {
		t.Errorf("top feeds = %+v", topFeeds)
	}
	adminEntries, err := models.GetAdminEntryStats(ctx, s.database, "2026-07-01", "2026-07-31")
	must(t, err)
	if adminEntries.TotalEntries != 3 {
		t.Errorf("admin entries = %d", adminEntries.TotalEntries)
	}
}

// find_neighbors: the oldest entry has a newer neighbour and no older one.
func (s *pgSmoke) checkNeighbors(oldest int64) {
	neighbors, err := entry.FindNeighbors(context.Background(), s.database, s.userID, oldest, &entry.Filter{})
	must(s.t, err)
	if neighbors.PrevID == nil {
		s.t.Error("oldest entry has a newer neighbour")
	}
	if neighbors.NextID != nil {
		s.t.Error("oldest entry has no older neighbour")
	}
}

// datetime('now')->now() shim and the snapshot read_after (to_char) comparison.
func (s *pgSmoke) checkReadSnapshot(target entry.WithFeed) {
	t, ctx := s.t, context.Background()
	changed, err := entry.SetReadForUser(ctx, s.database, s.userID, target.ID, true)
	must(t, err)
	if changed == nil || !changed.Changed {
		t.Fatal("marking read must change the entry")
	}
	refreshed, err := entry.FindByID(ctx, s.database, target.ID)
	must(t, err)
	if refreshed.ReadAt == nil {
		t.Error("read_at set via now()")
	}
	// A snapshot boundary just before the read instant keeps the just-read
	// entry in the snapshot: this is the to_char read_at comparison.
	snapshot := &entry.Filter{UnreadOnly: true, ReadAfter: ptr("2000-01-01 00:00:00")}
	visible := s.list(snapshot, &entry.ContinuationParams{Limit: 10, SortOrder: entry.SortPublishedAt})
	for _, row := range visible {
		if row.ID == target.ID {
			return
		}
	}
	t.Error("read-after snapshot must keep the just-read entry visible")
}

// mark_all_read_by_feed: the age cutoff (make_interval fork) and the
// timestamptz bind the SQLite run cannot prove.
func (s *pgSmoke) checkMarkAllReadBounds(target entry.WithFeed) {
	t, ctx := s.t, context.Background()
	markAll := func(limits entry.MarkAllRead) int64 {
		affected, err := entry.MarkAllReadByFeed(ctx, s.database, s.feedID, limits)
		must(t, err)
		return affected
	}
	if got := markAll(entry.MarkAllRead{OlderThanDays: ptr(int64(3650))}); got != 0 {
		t.Errorf("affected = %d, no entry is older than 3650 days", got)
	}
	year2000, year2100 := utc(t, "2000-01-01T00:00:00Z"), utc(t, "2100-01-01T00:00:00Z")
	if got := markAll(entry.MarkAllRead{StoredBy: &year2000}); got != 0 {
		t.Errorf("affected = %d, nothing was stored by the year 2000", got)
	}
	// The opposite bound proves the bind matches rows, not just excludes them.
	if got := markAll(entry.MarkAllRead{StoredBy: &year2100}); got == 0 {
		t.Error("everything was stored before the year 2100")
	}
	// Later checks need unread entries again; the target stays read.
	s.exec("UPDATE entry SET read_at = NULL WHERE id <> $1", target.ID)
}

// published_between / stored_before (timestamptz binds).
func (s *pgSmoke) checkTimestampBinds() {
	t, ctx := s.t, context.Background()
	year2000, nextDay, year2100 := utc(t, "2000-01-01T00:00:00Z"), utc(t, "2000-01-02T00:00:00Z"), utc(t, "2100-01-01T00:00:00Z")
	count := func(filter *entry.Filter) int64 {
		n, err := entry.CountByUser(ctx, s.database, s.userID, filter)
		must(t, err)
		return n
	}
	if count(&entry.Filter{PublishedBetween: &entry.TimeRange{Start: year2000, End: year2100}}) == 0 {
		t.Error("every entry was published or stored between 2000 and 2100")
	}
	if got := count(&entry.Filter{PublishedBetween: &entry.TimeRange{Start: year2000, End: nextDay}}); got != 0 {
		t.Errorf("count = %d, nothing was published on 2000-01-01", got)
	}
	marked, err := entry.MarkReadByFilter(ctx, s.database, s.userID, &entry.Filter{StoredBefore: &year2000})
	must(t, err)
	if marked != 0 {
		t.Errorf("marked = %d, nothing was stored by the year 2000", marked)
	}
}

// retention prune (column-driven make_interval fork): backdate the read entry
// 40 days and set 30-day retention so it becomes a prune victim.
func (s *pgSmoke) checkRetentionPrune(target entry.WithFeed) {
	t, ctx := s.t, context.Background()
	s.exec("UPDATE entry SET read_at = now() - make_interval(days => 40) WHERE id = $1", target.ID)
	must(t, models.UpdateRetentionReadDays(ctx, s.database, s.userID, 30))

	deleted, err := entry.PruneReadRetentionBatch(ctx, s.database, 100)
	must(t, err)
	if deleted != 1 {
		t.Errorf("deleted = %d, the 40-day-old read entry must be pruned", deleted)
	}
	if found, err := entry.FindByID(ctx, s.database, target.ID); err != nil || found != nil {
		t.Errorf("the pruned entry is still there: %v, %v", found, err)
	}
}

// offline reading: the settings upsert (ON CONFLICT DO NOTHING insert) and the
// two-query union, both rendered differently on this backend.
func (s *pgSmoke) checkOfflineSet() {
	t, ctx := s.t, context.Background()
	must(t, models.UpdateOfflineKeep(ctx, s.database, s.userID, 5))
	if keep, err := models.OfflineKeep(ctx, s.database, s.userID); err != nil || keep != 5 {
		t.Errorf("offline keep = %d, %v", keep, err)
	}
	offline, err := entry.ListOfflineSet(ctx, s.database, s.userID, 5)
	must(t, err)
	if len(offline) == 0 {
		t.Error("unread entries remain, so the offline set must not be empty")
	}
	if len(offline) > 5 {
		t.Errorf("the set holds %d entries, over the budget it was given", len(offline))
	}
	if none, err := entry.ListOfflineSet(ctx, s.database, s.userID, 0); err != nil || len(none) != 0 {
		t.Errorf("a zero budget is offline reading switched off, got %d, %v", len(none), err)
	}
}

// statistics: the DATE()/to_char bucket and the read_at range, in UTC and in a
// zone east of UTC where the local-day bounds are not midnight UTC.
func (s *pgSmoke) checkDailyReadCounts(ids []int64, pruned int64) {
	t, ctx := s.t, context.Background()
	// Read one of the remaining entries "today" for a non-empty daily bucket.
	survivor := ids[0]
	if survivor == pruned {
		survivor = ids[1]
	}
	if _, err := entry.SetReadForUser(ctx, s.database, s.userID, survivor, true); err != nil {
		t.Fatal(err)
	}
	taipei, err := time.LoadLocation("Asia/Taipei")
	must(t, err)
	for _, zone := range []*time.Location{time.UTC, taipei} {
		today := timezone.TodayIn(zone)
		counts, err := models.GetDailyReadCounts(ctx, s.database, models.DailyReadQuery{
			UserID: s.userID, From: today.String(), To: today.AddDays(1).String(), Loc: zone,
		})
		must(t, err)
		var total int64
		for _, count := range counts {
			total += count.Count
		}
		if total != 1 {
			t.Errorf("%s: total = %d, exactly one entry read today", zone, total)
		}
	}
}

// admin stats: pg_database_size and the MIN/MAX(created_at) to_char.
func (s *pgSmoke) checkAdminStats() {
	stats, err := models.GetAdminDatabaseStats(context.Background(), s.database)
	must(s.t, err)
	if stats.DBSizeBytes <= 0 {
		s.t.Error("pg_database_size must be positive")
	}
	if stats.TotalEntries != 2 {
		s.t.Errorf("total entries = %d, two remain after the prune", stats.TotalEntries)
	}
	// PG cannot measure reclaimable space without an extension; it must report
	// nil, never a zero that would read as a bloat-free database.
	if stats.Reclaimable != nil {
		s.t.Error("PG must not claim a reclaimable figure it cannot measure")
	}
}

// unread counts: a correlated COUNT subquery whose SUM is numeric on PG and
// must be cast back; a feed or category with nothing unread still reports 0.
func (s *pgSmoke) checkUnreadCounts(existingCategoryID int64) {
	t, ctx := s.t, context.Background()
	counted, err := models.CreateCategory(ctx, s.database, s.userID, "Counted")
	must(t, err)
	empty, err := models.CreateCategory(ctx, s.database, s.userID, "Empty")
	must(t, err)
	var feedIDs []int64
	for _, feedURL := range []string{"https://example.com/a.xml", "https://example.com/b.xml"} {
		created, err := models.CreateFeed(ctx, s.database, models.CreateFeedParams{CategoryID: counted.ID, URL: feedURL, Title: ptr("Counted")})
		must(t, err)
		feedIDs = append(feedIDs, created.ID)
	}
	busy, quiet := feedIDs[0], feedIDs[1]
	var busyEntries []int64
	for _, guid := range []string{"c1", "c2", "c3"} {
		result, err := entry.UpsertEntry(ctx, s.database, &entry.Upsert{FeedID: busy, GUID: guid})
		must(t, err)
		busyEntries = append(busyEntries, result.ID)
	}
	_, err = entry.MarkAsRead(ctx, s.database, busyEntries[0])
	must(t, err)

	byFeed, err := entry.CountUnreadByFeed(ctx, s.database, s.userID)
	must(t, err)
	wantCount(t, "by feed", byFeed, map[int64]int64{busy: 2, quiet: 0})
	inCategory, err := entry.CountUnreadByFeedInCategory(ctx, s.database, s.userID, counted.ID)
	must(t, err)
	if len(inCategory) != 2 {
		t.Errorf("in category = %v, want two feeds", inCategory)
	}
	wantCount(t, "in category", inCategory, map[int64]int64{busy: 2, quiet: 0})
	byCategory, err := entry.CountUnreadByCategory(ctx, s.database, s.userID)
	must(t, err)
	wantCount(t, "by category", byCategory, map[int64]int64{counted.ID: 2, empty.ID: 0})
	_ = existingCategoryID
}

func wantCount(t *testing.T, what string, got, want map[int64]int64) {
	t.Helper()
	for id, count := range want {
		if value, found := got[id]; !found || value != count {
			t.Errorf("%s: id %d = %d (present %v), want %d", what, id, value, found, count)
		}
	}
}

// the unique-violation mapping (is_unique_violation on PG).
func (s *pgSmoke) checkUniqueViolationMapping() {
	_, err := models.CreateCategory(context.Background(), s.database, s.userID, "Tech")
	if err != models.ErrCategoryExists {
		s.t.Errorf("a duplicate category must be ErrCategoryExists, got %v", err)
	}
}

// the /search boolean query language: is:unread with free text (ILIKE ESCAPE)
// and after:2026-01-01 (the EXTRACT(EPOCH FROM ...)::bigint fork for date bounds).
func (s *pgSmoke) checkSearchQueryLanguage() {
	t, ctx := s.t, context.Background()
	s.upsert("guid-rust-news", ptr("rust news"), nil)
	rustRead := s.upsert("guid-rust-read", ptr("rust read already"), nil)
	_, err := entry.SetReadForUser(ctx, s.database, s.userID, rustRead.ID, true)
	must(t, err)
	s.upsert("guid-go-news", ptr("go news"), nil)

	unread := s.titles(&entry.Filter{Query: mustParse(t, "is:unread rust")})
	if !unread["rust news"] {
		t.Error("is:unread rust must find the unread rust entry")
	}
	if unread["rust read already"] {
		t.Error("a read entry must be excluded by is:unread")
	}
	if unread["go news"] {
		t.Error("an entry without \"rust\" must be excluded by the free text")
	}

	oldAt, newAt := utc(t, "2025-06-01T00:00:00Z"), utc(t, "2026-03-01T00:00:00Z")
	s.upsert("guid-old", ptr("old"), &oldAt)
	s.upsert("guid-new", ptr("new"), &newAt)
	after := s.titles(&entry.Filter{Query: mustParse(t, "after:2026-01-01")})
	if !after["new"] || after["old"] {
		t.Errorf("after:2026-01-01 titles = %v, want new and not old", after)
	}
}

func mustParse(t *testing.T, input string) search.Node {
	t.Helper()
	node, err := search.Parse(input)
	must(t, err)
	return node
}

// session sweep: the bound-now comparison (TIMESTAMPTZ vs SQLite's TEXT) is why
// this needs no dialect fork.
func (s *pgSmoke) checkExpiredSessionSweep() {
	t, ctx := s.t, context.Background()
	newSession := models.NewSession{UserID: s.userID, UserAgent: "test-agent", IPAddress: "127.0.0.1"}
	expired, err := models.CreateSession(ctx, s.database, newSession)
	must(t, err)
	fresh, err := models.CreateSession(ctx, s.database, newSession)
	must(t, err)
	s.exec("UPDATE session SET expires_at = $1 WHERE id = $2", time.Now().Add(-time.Hour), expired.ID)

	swept, err := models.DeleteExpiredSessions(ctx, s.database)
	must(t, err)
	if swept != 1 {
		t.Errorf("swept = %d, only the backdated session must go", swept)
	}
	if found, _ := models.FindSessionByToken(ctx, s.database, expired.SessionToken); found != nil {
		t.Error("the expired session must be gone after the sweep")
	}
	if found, _ := models.FindSessionByToken(ctx, s.database, fresh.SessionToken); found == nil {
		t.Error("the unexpired session must survive the sweep")
	}
}

// api_token CRUD and delete_expired: TIMESTAMPTZ binding and the "user" FK.
func (s *pgSmoke) checkAPITokens() {
	t, ctx := s.t, context.Background()
	newToken := func(label string) *models.APIToken {
		created, err := models.CreateAPIToken(ctx, s.database, models.NewAPIToken{
			UserID: s.userID, Kind: "greader", Label: label, UserAgent: "test-agent", IPAddress: "127.0.0.1",
		})
		must(t, err)
		return created
	}
	exists := func(token string) bool {
		found, err := models.FindAPIToken(ctx, s.database, token)
		must(t, err)
		return found != nil
	}
	created := newToken("pg-client")
	if !strings.HasPrefix(created.Token, models.APITokenPrefix) {
		t.Errorf("token %q lacks the prefix", created.Token)
	}
	if !exists(created.Token) {
		t.Fatal("a just-created api_token must be found")
	}
	expired := newToken("pg-client-expired")
	s.exec("UPDATE api_token SET expires_at = $1 WHERE id = $2", time.Now().Add(-time.Hour), expired.ID)

	swept, err := models.DeleteExpiredTokens(ctx, s.database)
	must(t, err)
	if swept != 1 {
		t.Errorf("swept = %d, only the backdated api_token must go", swept)
	}
	if exists(expired.Token) {
		t.Error("the expired api_token must be gone after the sweep")
	}
	if !exists(created.Token) {
		t.Error("the unexpired api_token must survive the sweep")
	}

	// delete_token is user_id scoped, same as on SQLite.
	other, err := models.CreateUser(ctx, s.database, "pguser2", "hash", models.RoleUser)
	must(t, err)
	must(t, models.DeleteToken(ctx, s.database, created.ID, other.ID))
	if !exists(created.Token) {
		t.Error("delete_token must not remove a token owned by a different user")
	}
	must(t, models.DeleteToken(ctx, s.database, created.ID, s.userID))
	if exists(created.Token) {
		t.Error("the owner's delete must remove the token")
	}
}

// open tracking: the opt-in baseline compares entry.created_at to
// user_settings.pixel_tracking_enabled_at column to column, because a bound
// timestamp encodes differently on SQLite. Only a live server proves the pair
// agrees on PG.
func (s *pgSmoke) checkOpenTracking() {
	t, ctx := s.t, context.Background()
	must(t, models.UpdatePixelTracking(ctx, s.database, s.userID, true))
	enabledAt, err := models.PixelTrackingEnabledAt(ctx, s.database, s.userID)
	must(t, err)
	if enabledAt == nil {
		t.Fatal("enabling records a baseline")
	}

	// An entry created after the opt-in records; the seeded backlog does not.
	fresh, err := entry.UpsertEntry(ctx, s.database, &entry.Upsert{
		FeedID: s.feedID, GUID: "pg-pixel-fresh", Title: ptr("Fresh"),
		Link: ptr("https://example.com/pg-pixel-fresh"), Content: ptr("<p>Fresh.</p>"),
	})
	must(t, err)
	if recorded, err := models.RecordEntryOpen(ctx, s.database, s.userID, fresh.ID); err != nil || !recorded {
		t.Errorf("an entry created after the opt-in is inside the window: %v, %v", recorded, err)
	}
	if recorded, err := models.RecordEntryOpen(ctx, s.database, s.userID, fresh.ID); err != nil || recorded {
		t.Errorf("ON CONFLICT DO NOTHING makes a repeat hit a no-op: %v, %v", recorded, err)
	}
	rates, err := models.OpenRatesByFeed(ctx, s.database, s.userID)
	must(t, err)
	var rate *models.FeedOpenRate
	for i := range rates {
		if rates[i].FeedID == s.feedID {
			rate = &rates[i]
		}
	}
	if rate == nil {
		t.Fatal("the feed is listed once the reader has opted in")
	}
	if rate.Tracked != 1 || rate.Opened != 1 {
		t.Errorf("tracked, opened = %d, %d: only the post-opt-in entry is in the denominator", rate.Tracked, rate.Opened)
	}
	window, err := models.FindTrackingWindow(ctx, s.database, s.userID)
	must(t, err)
	if window.EnabledAt == nil || !window.EnabledAt.Equal(*enabledAt) {
		t.Errorf("window enabled at = %v, want %v", window.EnabledAt, enabledAt)
	}
	if window.TrackedSince() == nil {
		t.Error("an opted-in reader has a baseline to report")
	}

	// Re-saving the preferences form must not move the baseline forward.
	must(t, models.UpdatePixelTracking(ctx, s.database, s.userID, true))
	again, err := models.PixelTrackingEnabledAt(ctx, s.database, s.userID)
	must(t, err)
	if again == nil || !again.Equal(*enabledAt) {
		t.Errorf("baseline moved from %v to %v", enabledAt, again)
	}
	must(t, models.UpdatePixelTracking(ctx, s.database, s.userID, false))
}

// credential revocation is atomic with the password/status change: renaming
// api_token away makes the last step of each flow fail after the account row
// was already written inside the transaction.
func (s *pgSmoke) checkAtomicCredentialRevocation() {
	t, ctx := s.t, context.Background()
	holder, err := models.CreateUser(ctx, s.database, "holder", "old-hash", models.RoleUser)
	must(t, err)
	_, err = models.CreateSession(ctx, s.database, models.NewSession{UserID: holder.ID, UserAgent: "ua", IPAddress: "127.0.0.1"})
	must(t, err)
	s.exec("ALTER TABLE api_token RENAME TO api_token_off")

	if err := models.ChangePassword(ctx, s.database, holder.ID, "new-hash"); err == nil {
		t.Error("revocation fails without api_token, so the password change must fail")
	}
	if err := models.DisableUser(ctx, s.database, holder.ID); err == nil {
		t.Error("revocation fails without api_token, so disabling must fail")
	}

	s.exec("ALTER TABLE api_token_off RENAME TO api_token")
	unchanged, err := models.FindUserByID(ctx, s.database, holder.ID)
	must(t, err)
	if unchanged.PasswordHash != "old-hash" || unchanged.IsDisabled() {
		t.Errorf("the failed changes leaked: %+v", unchanged)
	}
	sessions, err := models.ListUserSessions(ctx, s.database, holder.ID)
	must(t, err)
	if len(sessions) != 1 {
		t.Errorf("sessions = %d, deletion must roll back with the failed change", len(sessions))
	}

	must(t, models.ChangePassword(ctx, s.database, holder.ID, "new-hash"))
	sessions, err = models.ListUserSessions(ctx, s.database, holder.ID)
	must(t, err)
	if len(sessions) != 0 {
		t.Errorf("sessions = %d after a successful password change", len(sessions))
	}
}

// concurrent first-run setup: LockUserTable admits exactly one. Without the
// table lock, READ COMMITTED lets every racer see an empty user table.
func (s *pgSmoke) checkConcurrentSetupAdmitsOne() {
	t, ctx := s.t, context.Background()
	s.exec(`TRUNCATE "user" RESTART IDENTITY CASCADE`)
	const racers = 8
	var winners atomic.Int64
	done := make(chan error, racers)
	for i := 0; i < racers; i++ {
		go func() { done <- s.raceToCreateAdmin(ctx, i, &winners) }()
	}
	for i := 0; i < racers; i++ {
		must(t, <-done)
	}
	if winners.Load() != 1 {
		t.Errorf("winners = %d, exactly one concurrent setup may create the admin", winners.Load())
	}
	if count, err := models.CountUsers(ctx, s.database); err != nil || count != 1 {
		t.Errorf("users = %d, %v", count, err)
	}
}

// raceToCreateAdmin mirrors the setup transaction.
func (s *pgSmoke) raceToCreateAdmin(ctx context.Context, index int, winners *atomic.Int64) error {
	tx, err := s.database.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()
	if err := tx.LockUserTable(ctx); err != nil {
		return err
	}
	count, err := models.CountUsers(ctx, tx)
	if err != nil || count != 0 {
		return err
	}
	// Widen the check-to-insert window so the race is deterministic.
	time.Sleep(300 * time.Millisecond)
	if _, err := models.CreateUser(ctx, tx, "admin"+string(rune('0'+index)), "hash", models.RoleAdmin); err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return err
	}
	winners.Add(1)
	return nil
}

// invite re-issue: the revoke rolls back with a failed insert. token_hash is
// unique, so reusing another account's token fails after the revoke ran. Also
// the orphaned feed image sweep.
func (s *pgSmoke) checkInviteReissueRollsBack() {
	t, ctx := s.t, context.Background()
	key := []byte("pg-invite-secret-at-least-32-bytes")
	alice, err := models.CreateUser(ctx, s.database, "alice", "!", models.RoleUser)
	must(t, err)
	bob, err := models.CreateUser(ctx, s.database, "bob", "!", models.RoleUser)
	must(t, err)
	issue := func(userID int64, token string) error {
		_, err := models.IssueInvite(ctx, s.database, models.IssueInviteParams{SecretKey: key, UserID: userID, Token: token})
		return err
	}
	must(t, issue(alice.ID, "old-link"))
	must(t, issue(bob.ID, "taken"))
	if err := issue(alice.ID, "taken"); err == nil {
		t.Error("a duplicate token hash must fail the insert")
	}
	if found, err := models.FindInviteByToken(ctx, s.database, key, "old-link"); err != nil || found == nil {
		t.Errorf("the old link must survive a failed re-issue: %v, %v", found, err)
	}

	// Orphaned feed images: one whose feed exists, one whose feed is gone.
	category, err := models.CreateCategory(ctx, s.database, alice.ID, "Tech")
	must(t, err)
	live, err := models.CreateFeed(ctx, s.database, models.CreateFeedParams{CategoryID: category.ID, URL: "https://example.com/orphan-sweep.xml", Title: ptr("Feed")})
	must(t, err)
	missingFeedID := live.ID + 1000
	for _, entityID := range []int64{live.ID, missingFeedID} {
		must(t, models.UpsertImage(ctx, s.database, models.ImageUpsert{
			EntityType: models.EntityFeed, EntityID: entityID, Data: []byte{1}, ContentType: "image/png",
		}))
	}
	if swept, err := models.DeleteOrphanedFeedImages(ctx, s.database, 500); err != nil || swept != 1 {
		t.Errorf("swept = %d, %v", swept, err)
	}
	for entityID, want := range map[int64]bool{live.ID: true, missingFeedID: false} {
		if got, err := models.ImageExists(ctx, s.database, models.EntityFeed, entityID); err != nil || got != want {
			t.Errorf("image for feed %d exists = %v, %v, want %v", entityID, got, err, want)
		}
	}
}
