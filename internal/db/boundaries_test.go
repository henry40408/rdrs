package db

import (
	"context"
	"errors"
	"log/slog"
	"sync"
	"testing"
	"time"
)

func inflightOf(d *DB) int {
	d.sched.mu.Lock()
	defer d.sched.mu.Unlock()
	return d.sched.inflight
}

func TestAdmissionCountsOnlyUserOperationsOnSQLite(t *testing.T) {
	database, _ := openTemp(t)
	release, err := database.admit(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if got := inflightOf(database); got != 1 {
		t.Errorf("in flight during a user operation = %d, want 1", got)
	}
	release()
	if got := inflightOf(database); got != 0 {
		t.Errorf("in flight after release = %d, want 0", got)
	}

	background := database.Background()
	releaseBackground, err := background.admit(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer releaseBackground()
	if got := inflightOf(database); got != 0 {
		t.Errorf("a background operation counted as in flight: %d", got)
	}
}

func TestBackgroundAdmissionWaitsForAUserOperationOnSQLite(t *testing.T) {
	database, _ := openTemp(t)
	releaseUser, err := database.admit(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	if _, err := database.Background().admit(ctx); !errors.Is(err, context.DeadlineExceeded) {
		t.Errorf("background admit during a user operation = %v, want a deadline error", err)
	}
	releaseUser()
	if _, err := database.Background().admit(context.Background()); err != nil {
		t.Errorf("background admit once idle = %v", err)
	}
}

func TestPostgresAdmissionNeverWaits(t *testing.T) {
	postgres := &DB{dialect: DialectPostgres, sched: newSched(), priority: User}
	release, err := postgres.admit(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	release()
	if got := inflightOf(postgres); got != 0 {
		t.Errorf("postgres counted a user operation: %d", got)
	}

	holdUser := postgres.sched.enterUser()
	defer holdUser()
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if _, err := postgres.Background().admit(ctx); err != nil {
		t.Errorf("a background operation waited on postgres: %v", err)
	}
}

type logged struct {
	level   slog.Level
	message string
}

type recordingHandler struct {
	mu      *sync.Mutex
	records *[]logged
}

func (h recordingHandler) Enabled(context.Context, slog.Level) bool { return true }
func (h recordingHandler) Handle(_ context.Context, r slog.Record) error {
	h.mu.Lock()
	defer h.mu.Unlock()
	*h.records = append(*h.records, logged{r.Level, r.Message})
	return nil
}
func (h recordingHandler) WithAttrs([]slog.Attr) slog.Handler { return h }
func (h recordingHandler) WithGroup(string) slog.Handler      { return h }

func recordLogs(t *testing.T) *[]logged {
	t.Helper()
	previous := slog.Default()
	records := &[]logged{}
	slog.SetDefault(slog.New(recordingHandler{mu: &sync.Mutex{}, records: records}))
	t.Cleanup(func() { slog.SetDefault(previous) })
	return records
}

func TestShutdownCheckpointsTheWALAndSaysSo(t *testing.T) {
	records := recordLogs(t)
	database, _ := openTemp(t)
	if err := database.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(*records) != 1 || (*records)[0].level != slog.LevelInfo {
		t.Errorf("a clean shutdown logged %+v, want one info record for the checkpoint", *records)
	}
}

func TestShutdownLogsAFailedCheckpointButStillCloses(t *testing.T) {
	records := recordLogs(t)
	database, _ := openTemp(t)
	cancelled, cancel := context.WithCancel(context.Background())
	cancel()
	if err := database.Shutdown(cancelled); err != nil {
		t.Fatal(err)
	}
	var errorRecords int
	for _, record := range *records {
		if record.level == slog.LevelError {
			errorRecords++
		}
	}
	if errorRecords != 1 {
		t.Errorf("failed checkpoint logged %d error records: %+v", errorRecords, *records)
	}
}

func TestMaintenanceLeavesALightlyFragmentedFileAlone(t *testing.T) {
	database, _ := openTemp(t)
	mustExec(t, database, "CREATE TABLE blob (id INTEGER PRIMARY KEY, b BLOB)")
	for range 40 {
		mustExec(t, database, "INSERT INTO blob (b) VALUES (zeroblob(65536))")
	}
	// Two of forty rows is about 5% free pages, under the 20% trigger.
	mustExec(t, database, "DELETE FROM blob WHERE id <= 2")
	if freelist := scanInt(t, database, "PRAGMA freelist_count"); freelist == 0 {
		t.Fatal("the fixture freed nothing")
	}

	vacuumed, err := database.RunMaintenance(context.Background())
	if err != nil || vacuumed {
		t.Errorf("RunMaintenance = %v, %v; want no VACUUM below the trigger", vacuumed, err)
	}
}
