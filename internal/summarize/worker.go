package summarize

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"sync"
	"time"

	"github.com/henry40408/rdrs/internal/cache"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/events"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/save"
)

const (
	// Timeout is the hard limit per Kagi request, so a hung call cannot block
	// the single worker. Do not lower it without confirming Kagi's worst-case
	// latency.
	Timeout = 90 * time.Second
	// QueueSize bounds the queue: a full channel fails the enqueue fast, and the
	// caller relies on the pending database row.
	QueueSize = 100
	// EnqueueTimeout is how long a handler waits for room in the queue.
	EnqueueTimeout = 5 * time.Second
)

// enqueueTimeout is a variable so a test need not wait out the real limit.
var enqueueTimeout = EnqueueTimeout

// Job is one entry to summarize.
type Job struct {
	UserID    int64
	EntryID   int64
	EntryLink string
}

// NewQueue makes the job channel.
func NewQueue() chan Job {
	return make(chan Job, QueueSize)
}

// Enqueue waits up to EnqueueTimeout for room in the queue.
func Enqueue(ctx context.Context, queue chan<- Job, job Job) error {
	timer := time.NewTimer(enqueueTimeout)
	defer timer.Stop()
	select {
	case queue <- job:
		return nil
	case <-timer.C:
		return ErrQueueFull
	case <-ctx.Done():
		return ctx.Err()
	}
}

// ErrQueueFull is returned by Enqueue when the queue stays full.
var ErrQueueFull = errors.New("summary queue is full")

type jobKey struct{ userID, entryID int64 }

// Cancels holds the cancel function of each running job. The cancel handler
// calls Cancel; the worker registers on dequeue and unregisters when done.
type Cancels struct {
	mu      sync.Mutex
	running map[jobKey]*registration
}

type registration struct{ cancel context.CancelFunc }

// NewCancels makes an empty registry.
func NewCancels() *Cancels {
	return &Cancels{running: make(map[jobKey]*registration)}
}

// Cancel stops the job if it is running. A job still queued is stopped by its
// deleted database row instead.
func (c *Cancels) Cancel(userID, entryID int64) {
	c.mu.Lock()
	current := c.running[jobKey{userID, entryID}]
	delete(c.running, jobKey{userID, entryID})
	c.mu.Unlock()
	if current != nil {
		current.cancel()
	}
}

// register returns the job's context and a function that unregisters it. Only
// the registration that is still current is removed, so a Cancel followed by a
// new job for the same entry is not undone.
func (c *Cancels) register(key jobKey) (context.Context, func()) {
	ctx, cancel := context.WithCancel(context.Background())
	current := &registration{cancel: cancel}
	c.mu.Lock()
	c.running[key] = current
	c.mu.Unlock()
	return ctx, func() {
		c.mu.Lock()
		if c.running[key] == current {
			delete(c.running, key)
		}
		c.mu.Unlock()
		cancel()
	}
}

// Worker drains the job queue one job at a time: Kagi is rate limited per key.
type Worker struct {
	DB      *db.DB
	Cache   *cache.Summary
	Sidebar *cache.Sidebar
	Cancels *Cancels
	Events  *events.Bus
	// ServiceTokenKey opens the stored Kagi credential; nil when RDRS_SECRET is
	// generated and credentials are stored in the clear.
	ServiceTokenKey []byte
	Client          *http.Client
	Retry           fetch.RetryConfig
	// Timeout overrides the Kagi limit; zero means Timeout.
	Timeout time.Duration
}

// NewWorker fills in the HTTP client and retry policy the application uses.
func NewWorker(worker Worker) *Worker {
	worker.DB = worker.DB.Background()
	worker.Client = fetch.NewExternalAPIClient()
	worker.Retry = fetch.DefaultRetry()
	return &worker
}

// Run processes jobs until ctx ends, then finishes what is already queued. The
// jobs themselves run on a context that outlives ctx so the drain can write.
func (w *Worker) Run(ctx context.Context, jobs <-chan Job) {
	slog.Info("summary worker started", "event", "summary.worker_started")
	defer slog.Info("summary worker stopped", "event", "summary.worker_stopped")
	work := context.WithoutCancel(ctx)
	for {
		select {
		case <-ctx.Done():
			slog.Info("summary worker stopping, draining remaining jobs", "event", "summary.worker_stopping")
			drainCtx, cancel := context.WithTimeout(work, DrainTimeout)
			w.drain(drainCtx, jobs)
			cancel()
			return
		case job, open := <-jobs:
			if !open {
				return
			}
			w.process(work, job)
		}
	}
}

// DrainTimeout bounds how long shutdown waits for queued jobs, like the 30
// second join timeout of the Rust server. Rows left unfinished are picked up by
// RecoverIncomplete on the next start.
const DrainTimeout = 30 * time.Second

func (w *Worker) drain(ctx context.Context, jobs <-chan Job) {
	for ctx.Err() == nil {
		select {
		case job, open := <-jobs:
			if !open {
				return
			}
			w.process(ctx, job)
		default:
			return
		}
	}
}

func (w *Worker) process(ctx context.Context, job Job) {
	key := jobKey{job.UserID, job.EntryID}
	jobCtx, unregister := w.Cancels.register(key)
	defer unregister()
	// The drain deadline must reach a Kagi call in flight, not only the gap
	// between jobs; the row is left for RecoverIncomplete.
	defer context.AfterFunc(ctx, unregister)()
	w.run(ctx, jobCtx, job)
}

// failureOf records a failure the job itself caused, as opposed to Kagi
// failing, in the cache, the database and the event stream.
func (w *Worker) failureOf(ctx context.Context, job Job, message string) {
	w.Cache.SetFailed(job.UserID, job.EntryID, message)
	_, _ = models.SetSummaryFailed(ctx, w.DB, job.UserID, job.EntryID, message)
	w.emitStatus(job, models.SummaryFailed)
}

func (w *Worker) emitStatus(job Job, status models.SummaryStatus) {
	w.Events.EmitSummary(job.UserID, job.EntryID, &status)
}

// loadKagiConfig returns the user's Kagi settings, or the message to fail the
// job with.
func (w *Worker) loadKagiConfig(ctx context.Context, job Job) (KagiConfig, string) {
	stored, err := models.SaveServicesOf(ctx, w.DB, job.UserID, w.ServiceTokenKey)
	if err != nil {
		slog.Error("failed to load user settings", "event", "summary.settings_load_failed",
			"user_id", job.UserID, "entry_id", job.EntryID, "error", err.Error())
		return KagiConfig{}, "Failed to load Kagi settings"
	}
	config := save.ConfigOf(stored).Kagi
	if config == nil || !config.IsConfigured() {
		return KagiConfig{}, "Kagi is not configured"
	}
	return *config, ""
}

func (w *Worker) run(ctx, jobCtx context.Context, job Job) {
	slog.Debug("processing summary job", "event", "summary.processing",
		"user_id", job.UserID, "entry_id", job.EntryID, "link", job.EntryLink)

	// A missing row means the job was cancelled while queued: stop without
	// repopulating the cache, or the summary would be resurrected.
	if err := models.SetSummaryProcessing(ctx, w.DB, job.UserID, job.EntryID); errors.Is(err, models.ErrSummaryNotFound) {
		w.Cache.Remove(job.UserID, job.EntryID)
		return
	}
	w.Cache.SetProcessing(job.UserID, job.EntryID)
	w.emitStatus(job, models.SummaryProcessing)

	config, message := w.loadKagiConfig(ctx, job)
	if message != "" {
		w.failureOf(ctx, job, message)
		return
	}
	outcome := w.summarize(jobCtx, config, job.EntryLink)
	switch {
	case outcome.cancelled:
		// The cancel handler owns cleanup; write nothing back.
		slog.Debug("summary cancelled", "event", "summary.cancelled", "user_id", job.UserID, "entry_id", job.EntryID)
	case outcome.failure != "":
		w.storeFailed(ctx, job, outcome.failure)
	default:
		w.storeCompleted(ctx, job, outcome.text)
	}
}

// outcome is how a summarization ended: cancelled, failed with a message, or
// completed with text.
type outcome struct {
	cancelled bool
	failure   string
	text      string
}

// summarize races Kagi against cancellation and the hard timeout. Cancellation
// wins over any result that arrives with it.
func (w *Worker) summarize(jobCtx context.Context, config KagiConfig, link string) outcome {
	limit := w.Timeout
	if limit == 0 {
		limit = Timeout
	}
	ctx, stop := context.WithTimeout(jobCtx, limit)
	defer stop()
	text, failure := w.askKagi(ctx, config, link)
	switch {
	case jobCtx.Err() != nil:
		return outcome{cancelled: true}
	case failure != "" && ctx.Err() != nil:
		return outcome{failure: "Summarization timed out"}
	}
	return outcome{failure: failure, text: text}
}

// askKagi returns the summary text, or the message to fail the job with.
func (w *Worker) askKagi(ctx context.Context, config KagiConfig, link string) (text, failure string) {
	result, err := Summarize(ctx, Request{Config: config, URL: link, Client: w.Client, Retry: w.Retry})
	switch {
	case err != nil:
		return "", "Kagi API error: " + err.Error()
	case !result.Success && result.Error != nil:
		return "", *result.Error
	case !result.Success:
		return "", "Unknown error"
	case result.OutputText == nil:
		return "", "No summary text returned"
	}
	return *result.OutputText, ""
}

func (w *Worker) storeCompleted(ctx context.Context, job Job, text string) {
	slog.Debug("summary completed", "event", "summary.completed",
		"user_id", job.UserID, "entry_id", job.EntryID, "chars", len(text))
	_, err := models.SetSummaryCompleted(ctx, w.DB, job.UserID, job.EntryID, text)
	if errors.Is(err, models.ErrSummaryNotFound) {
		w.Cache.Remove(job.UserID, job.EntryID)
		return
	}
	w.Cache.SetCompleted(job.UserID, job.EntryID, text)
	// The sidebar's "Summarized" badge ticks.
	w.Sidebar.Bust(job.UserID)
	w.emitStatus(job, models.SummaryCompleted)
	w.Events.EmitSidebar(job.UserID)
}

func (w *Worker) storeFailed(ctx context.Context, job Job, message string) {
	slog.Warn("summary failed", "event", "summary.failed",
		"user_id", job.UserID, "entry_id", job.EntryID, "error", message)
	_, err := models.SetSummaryFailed(ctx, w.DB, job.UserID, job.EntryID, message)
	if errors.Is(err, models.ErrSummaryNotFound) {
		w.Cache.Remove(job.UserID, job.EntryID)
		return
	}
	w.Cache.SetFailed(job.UserID, job.EntryID, message)
	w.emitStatus(job, models.SummaryFailed)
}

// RecoverIncomplete queues the pending and processing rows a shutdown left
// behind and returns how many it found.
func RecoverIncomplete(ctx context.Context, database *db.DB, queue chan<- Job, summaries *cache.Summary) int {
	incomplete, err := models.FindIncompleteSummaries(ctx, database.Background())
	if err != nil {
		slog.Error("failed to find incomplete summary jobs", "event", "summary.recovery_failed", "error", err.Error())
		return 0
	}
	if len(incomplete) > 0 {
		slog.Info("recovering incomplete summary jobs", "event", "summary.recovering", "count", len(incomplete))
	}
	for _, row := range incomplete {
		summaries.SetPending(row.UserID, row.EntryID)
		select {
		case queue <- Job{UserID: row.UserID, EntryID: row.EntryID, EntryLink: row.Link}:
		case <-ctx.Done():
			slog.Error("failed to re-queue summary job", "event", "summary.requeue_failed",
				"user_id", row.UserID, "entry_id", row.EntryID, "error", ctx.Err().Error())
			return len(incomplete)
		}
	}
	return len(incomplete)
}
