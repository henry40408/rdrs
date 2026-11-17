package summarize

import (
	"context"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/cache"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/events"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/migrate"
	"github.com/henry40408/rdrs/internal/models"
)

func TestMain(m *testing.M) {
	slog.SetDefault(slog.New(slog.DiscardHandler))
	os.Exit(m.Run())
}

type workerFixture struct {
	worker *Worker
	userID int64
	feedID int64
	events *events.Subscription
}

func openDB(t *testing.T) *db.DB {
	t.Helper()
	ctx := context.Background()
	database, err := db.Open(ctx, filepath.Join(t.TempDir(), "test.sqlite3"), config.SQLite)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Shutdown(ctx) })
	if err := migrate.Run(ctx, database); err != nil {
		t.Fatal(err)
	}
	return database
}

const kagiSettings = `{"kagi":{"session_token":"tok"}}`

// newWorkerFixture starts a fake Kagi answering with handler and stores a Kagi
// token for the user, unless settings is empty.
func newWorkerFixture(t *testing.T, settings string, handler http.HandlerFunc) *workerFixture {
	t.Helper()
	ctx := context.Background()
	database := openDB(t)
	user, err := models.CreateUser(ctx, database, "u", "hash", models.RoleUser)
	if err != nil {
		t.Fatal(err)
	}
	category, err := models.CreateCategory(ctx, database, user.ID, "c")
	if err != nil {
		t.Fatal(err)
	}
	feed, err := models.CreateFeed(ctx, database, models.CreateFeedParams{CategoryID: category.ID, URL: "https://example.com/f.xml"})
	if err != nil {
		t.Fatal(err)
	}
	if settings != "" {
		if _, err := models.UpdateSaveServices(ctx, database, user.ID, settings, nil); err != nil {
			t.Fatal(err)
		}
	}
	kagi := httptest.NewServer(handler)
	t.Cleanup(kagi.Close)
	t.Setenv("RDRS_KAGI_API_BASE", kagi.URL)
	bus := events.NewBus()
	worker := NewWorker(Worker{
		DB: database, Cache: cache.NewSummary(10, time.Hour), Sidebar: cache.NewSidebar(10, time.Hour),
		Cancels: NewCancels(), Events: bus,
	})
	worker.Client = kagi.Client()
	worker.Retry = noRetry
	return &workerFixture{worker: worker, userID: user.ID, feedID: feed.ID, events: bus.Subscribe()}
}

func (f *workerFixture) addEntry(t *testing.T, guid string) Job {
	t.Helper()
	ctx := context.Background()
	if _, err := f.worker.DB.ExecContext(ctx,
		`INSERT INTO entry (feed_id, guid, title, link) VALUES ($1, $2, 'T', 'https://example.com/e')`, f.feedID, guid); err != nil {
		t.Fatal(err)
	}
	var id int64
	if err := f.worker.DB.QueryRowContext(ctx, `SELECT id FROM entry WHERE guid = $1`, guid).Scan(&id); err != nil {
		t.Fatal(err)
	}
	return Job{UserID: f.userID, EntryID: id, EntryLink: "https://example.com/e"}
}

func (f *workerFixture) queue(t *testing.T, job Job) {
	t.Helper()
	if _, err := models.UpsertPendingSummary(context.Background(), f.worker.DB, job.UserID, job.EntryID); err != nil {
		t.Fatal(err)
	}
	f.worker.Cache.SetPending(job.UserID, job.EntryID)
}

func (f *workerFixture) row(t *testing.T, job Job) *models.EntrySummary {
	t.Helper()
	row, err := models.FindEntrySummary(context.Background(), f.worker.DB, job.UserID, job.EntryID)
	if err != nil {
		t.Fatal(err)
	}
	return row
}

// drainEvents returns the events already emitted.
func (f *workerFixture) drainEvents(t *testing.T) []events.Event {
	t.Helper()
	var got []events.Event
	for {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
		event, err := f.events.Next(ctx)
		cancel()
		if err != nil {
			return got
		}
		got = append(got, event)
	}
}

func summaryEvent(e events.Event) string {
	if e.Kind != events.Summary || e.Status == nil {
		return "other"
	}
	return string(*e.Status)
}

func statuses(list []events.Event) []string {
	var out []string
	for _, e := range list {
		if e.Kind == events.Sidebar {
			out = append(out, "sidebar")
			continue
		}
		out = append(out, summaryEvent(e))
	}
	return out
}

func equal(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

func answer(markdown string) http.HandlerFunc {
	return func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte(`{"output_data":{"markdown":` + quote(markdown) + `}}`))
	}
}

func quote(s string) string {
	out := `"`
	for _, r := range s {
		if r == '\n' {
			out += `\n`
			continue
		}
		out += string(r)
	}
	return out + `"`
}

func TestCompletedJobStoresTheSummary(t *testing.T) {
	f := newWorkerFixture(t, kagiSettings, answer("Title: T\n\nThe body."))
	job := f.addEntry(t, "a")
	f.queue(t, job)
	f.worker.Sidebar.InsertIfCurrent(f.userID, f.worker.Sidebar.BeginRead(f.userID), cache.Chrome{})

	f.worker.process(context.Background(), job)

	row := f.row(t, job)
	if row.Status != models.SummaryCompleted || row.SummaryText == nil || *row.SummaryText != "The body." {
		t.Fatalf("row = %+v", row)
	}
	if entry, _ := f.worker.Cache.Get(job.UserID, job.EntryID); entry.Status != models.SummaryCompleted || *entry.SummaryText != "The body." {
		t.Errorf("cache = %+v", entry)
	}
	if f.worker.Sidebar.Get(f.userID) != nil {
		t.Error("the sidebar cache should be busted so the Summarized badge ticks")
	}
	if got := statuses(f.drainEvents(t)); !equal(got, []string{"processing", "completed", "sidebar"}) {
		t.Errorf("events = %v", got)
	}
}

func TestKagiNotConfiguredFailsTheJob(t *testing.T) {
	for name, settings := range map[string]string{
		"no settings":   "",
		"no kagi":       `{"linkding":null}`,
		"empty token":   `{"kagi":{"session_token":""}}`,
		"broken json":   `{`,
		"unknown shape": `[]`,
	} {
		t.Run(name, func(t *testing.T) {
			f := newWorkerFixture(t, settings, answer("never asked"))
			job := f.addEntry(t, "a")
			f.queue(t, job)
			f.worker.process(context.Background(), job)
			row := f.row(t, job)
			if row.Status != models.SummaryFailed || *row.ErrorMessage != "Kagi is not configured" {
				t.Fatalf("row = %+v", row)
			}
			if entry, _ := f.worker.Cache.Get(job.UserID, job.EntryID); entry.Status != models.SummaryFailed {
				t.Errorf("cache = %+v", entry)
			}
			if got := statuses(f.drainEvents(t)); !equal(got, []string{"processing", "failed"}) {
				t.Errorf("events = %v", got)
			}
		})
	}
}

func TestKagiFailureMessagesBecomeTheJobError(t *testing.T) {
	for name, c := range map[string]struct {
		handler http.HandlerFunc
		want    string
	}{
		"body error": {func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte(`{"error":"nope"}`)) }, "nope"},
		"status":     {func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(401) }, "Invalid session token"},
		"no summary": {func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte(`{}`)) }, "No summary returned from Kagi"},
		"bad json": {func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte(`{`)) },
			"Kagi API error: Failed to parse Kagi response: unexpected EOF"},
	} {
		t.Run(name, func(t *testing.T) {
			f := newWorkerFixture(t, kagiSettings, c.handler)
			job := f.addEntry(t, "a")
			f.queue(t, job)
			f.worker.process(context.Background(), job)
			row := f.row(t, job)
			if row.Status != models.SummaryFailed || *row.ErrorMessage != c.want {
				t.Fatalf("row = %+v, want error %q", row, c.want)
			}
			if entry, _ := f.worker.Cache.Get(job.UserID, job.EntryID); *entry.ErrorMessage != c.want {
				t.Errorf("cache = %+v", entry)
			}
			if got := statuses(f.drainEvents(t)); !equal(got, []string{"processing", "failed"}) {
				t.Errorf("events = %v", got)
			}
		})
	}
}

func TestAJobWhoseRowWasDeletedWritesNothing(t *testing.T) {
	f := newWorkerFixture(t, kagiSettings, answer("x"))
	job := f.addEntry(t, "a")
	f.worker.Cache.SetPending(job.UserID, job.EntryID) // the cancel handler had deleted the row
	f.worker.process(context.Background(), job)
	if _, found := f.worker.Cache.Get(job.UserID, job.EntryID); found {
		t.Error("the cache must not resurrect a cancelled summary")
	}
	if f.row(t, job) != nil {
		t.Error("no row should appear")
	}
	if got := f.drainEvents(t); len(got) != 0 {
		t.Errorf("events = %v", statuses(got))
	}
}

func TestRowDeletedWhileKagiRuns(t *testing.T) {
	var f *workerFixture
	var job Job
	f = newWorkerFixture(t, kagiSettings, func(w http.ResponseWriter, _ *http.Request) {
		_, _ = models.DeleteEntrySummary(context.Background(), f.worker.DB, job.UserID, job.EntryID)
		answer("late")(w, nil)
	})
	job = f.addEntry(t, "a")
	f.queue(t, job)
	f.worker.process(context.Background(), job)
	if _, found := f.worker.Cache.Get(job.UserID, job.EntryID); found {
		t.Error("completing a deleted row must drop the cache entry")
	}
	if got := statuses(f.drainEvents(t)); !equal(got, []string{"processing"}) {
		t.Errorf("events = %v", got)
	}
}

func TestFailureForADeletedRowDropsTheCacheEntry(t *testing.T) {
	var f *workerFixture
	var job Job
	f = newWorkerFixture(t, kagiSettings, func(w http.ResponseWriter, _ *http.Request) {
		_, _ = models.DeleteEntrySummary(context.Background(), f.worker.DB, job.UserID, job.EntryID)
		w.WriteHeader(403)
	})
	job = f.addEntry(t, "a")
	f.queue(t, job)
	f.worker.process(context.Background(), job)
	if _, found := f.worker.Cache.Get(job.UserID, job.EntryID); found {
		t.Error("failing a deleted row must drop the cache entry")
	}
}

func TestCancelStopsARunningJobWithoutWriting(t *testing.T) {
	started := make(chan struct{})
	f := newWorkerFixture(t, kagiSettings, func(_ http.ResponseWriter, r *http.Request) {
		close(started)
		<-r.Context().Done()
	})
	job := f.addEntry(t, "a")
	f.queue(t, job)
	done := make(chan struct{})
	go func() {
		defer close(done)
		f.worker.process(context.Background(), job)
	}()
	<-started
	f.worker.Cancels.Cancel(job.UserID, job.EntryID)
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("cancel did not stop the job")
	}
	if row := f.row(t, job); row.Status != models.SummaryProcessing {
		t.Errorf("a cancelled job writes nothing back, row = %+v", row)
	}
	if got := statuses(f.drainEvents(t)); !equal(got, []string{"processing"}) {
		t.Errorf("events = %v", got)
	}
}

func TestSlowKagiTimesOut(t *testing.T) {
	f := newWorkerFixture(t, kagiSettings, func(_ http.ResponseWriter, r *http.Request) { <-r.Context().Done() })
	f.worker.Timeout = 50 * time.Millisecond
	job := f.addEntry(t, "a")
	f.queue(t, job)
	f.worker.process(context.Background(), job)
	if row := f.row(t, job); row.Status != models.SummaryFailed || *row.ErrorMessage != "Summarization timed out" {
		t.Fatalf("row = %+v", row)
	}
}

func TestCancelsUnregisterWhenTheJobEnds(t *testing.T) {
	f := newWorkerFixture(t, kagiSettings, answer("x"))
	job := f.addEntry(t, "a")
	f.queue(t, job)
	f.worker.process(context.Background(), job)
	if len(f.worker.Cancels.running) != 0 {
		t.Fatalf("registry = %v", f.worker.Cancels.running)
	}
	f.worker.Cancels.Cancel(job.UserID, job.EntryID) // nothing to cancel
}

func TestUnregisterKeepsANewerRegistration(t *testing.T) {
	cancels := NewCancels()
	key := jobKey{1, 2}
	_, first := cancels.register(key)
	cancels.Cancel(1, 2)
	secondCtx, second := cancels.register(key)
	first()
	if cancels.running[key] == nil || secondCtx.Err() != nil {
		t.Fatal("the first job's cleanup must not remove the second job's registration")
	}
	second()
	if len(cancels.running) != 0 {
		t.Fatal("second registration should be gone")
	}
}

func TestRunDrainsQueuedJobsOnShutdown(t *testing.T) {
	f := newWorkerFixture(t, kagiSettings, answer("body"))
	queue := NewQueue()
	first, second := f.addEntry(t, "a"), f.addEntry(t, "b")
	for _, job := range []Job{first, second} {
		f.queue(t, job)
		queue <- job
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	f.worker.Run(ctx, queue)
	for _, job := range []Job{first, second} {
		if row := f.row(t, job); row.Status != models.SummaryCompleted {
			t.Errorf("job %d: row = %+v", job.EntryID, row)
		}
	}
}

func TestRunStopsWhenTheQueueIsClosed(t *testing.T) {
	f := newWorkerFixture(t, kagiSettings, answer("body"))
	queue := make(chan Job)
	close(queue)
	finished := make(chan struct{})
	go func() {
		f.worker.Run(context.Background(), queue)
		close(finished)
	}()
	select {
	case <-finished:
	case <-time.After(5 * time.Second):
		t.Fatal("Run spins on a closed queue")
	}
}

func TestRunProcessesJobsWhileRunning(t *testing.T) {
	f := newWorkerFixture(t, kagiSettings, answer("body"))
	queue := NewQueue()
	job := f.addEntry(t, "a")
	f.queue(t, job)
	ctx, cancel := context.WithCancel(context.Background())
	finished := make(chan struct{})
	go func() {
		defer close(finished)
		f.worker.Run(ctx, queue)
	}()
	queue <- job
	deadline := time.Now().Add(5 * time.Second)
	for f.row(t, job).Status != models.SummaryCompleted {
		if time.Now().After(deadline) {
			t.Fatal("job was not processed")
		}
		time.Sleep(10 * time.Millisecond)
	}
	cancel()
	<-finished
}

func TestRecoverIncompleteRequeuesPendingAndProcessing(t *testing.T) {
	f := newWorkerFixture(t, kagiSettings, answer("x"))
	pending, processing, completed := f.addEntry(t, "a"), f.addEntry(t, "b"), f.addEntry(t, "c")
	ctx := context.Background()
	for _, job := range []Job{pending, processing, completed} {
		f.queue(t, job)
	}
	if err := models.SetSummaryProcessing(ctx, f.worker.DB, processing.UserID, processing.EntryID); err != nil {
		t.Fatal(err)
	}
	if _, err := models.SetSummaryCompleted(ctx, f.worker.DB, completed.UserID, completed.EntryID, "done"); err != nil {
		t.Fatal(err)
	}
	f.worker.Cache.Remove(pending.UserID, pending.EntryID)
	queue := NewQueue()

	count := RecoverIncomplete(ctx, f.worker.DB, queue, f.worker.Cache)

	if count != 2 || len(queue) != 2 {
		t.Fatalf("count = %d, queued = %d", count, len(queue))
	}
	if status, _ := f.worker.Cache.GetStatus(pending.UserID, pending.EntryID); status != models.SummaryPending {
		t.Errorf("recovered job should show as pending, got %q", status)
	}
	got := map[int64]string{}
	for range 2 {
		job := <-queue
		got[job.EntryID] = job.EntryLink
	}
	if got[pending.EntryID] != "https://example.com/e" || got[processing.EntryID] != "https://example.com/e" {
		t.Errorf("queued = %v", got)
	}
}

func TestRecoverIncompleteStopsWhenShutdownInterrupts(t *testing.T) {
	f := newWorkerFixture(t, kagiSettings, answer("x"))
	f.queue(t, f.addEntry(t, "a"))
	ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
	defer cancel()
	queue := make(chan Job) // nobody reads, so the send waits for shutdown
	if count := RecoverIncomplete(ctx, f.worker.DB, queue, f.worker.Cache); count != 1 {
		t.Fatalf("count = %d", count)
	}
}

func TestRecoverIncompleteWithNothingToDo(t *testing.T) {
	f := newWorkerFixture(t, kagiSettings, answer("x"))
	if count := RecoverIncomplete(context.Background(), f.worker.DB, NewQueue(), f.worker.Cache); count != 0 {
		t.Fatalf("count = %d", count)
	}
}

func TestEnqueue(t *testing.T) {
	queue := make(chan Job, 1)
	if err := Enqueue(context.Background(), queue, Job{EntryID: 1}); err != nil {
		t.Fatal(err)
	}
	enqueueTimeout = 20 * time.Millisecond
	defer func() { enqueueTimeout = EnqueueTimeout }()
	if err := Enqueue(context.Background(), queue, Job{EntryID: 2}); err != ErrQueueFull {
		t.Fatalf("full queue: err = %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err := Enqueue(ctx, queue, Job{EntryID: 3}); err != context.Canceled {
		t.Fatalf("cancelled: err = %v", err)
	}
	if len(queue) != 1 {
		t.Fatalf("queued = %d", len(queue))
	}
}

func TestNewWorkerUsesTheBackgroundHandleAndExternalTimeout(t *testing.T) {
	database := openDB(t)
	worker := NewWorker(Worker{DB: database})
	if worker.DB == database {
		t.Error("the worker should use the background handle")
	}
	if worker.Client.Timeout != fetch.ExternalAPITimeout || worker.Retry != fetch.DefaultRetry() {
		t.Errorf("client timeout = %v, retry = %+v", worker.Client.Timeout, worker.Retry)
	}
}
