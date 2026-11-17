package summarize

import (
	"context"
	"net/http"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/models"
)

// The drain deadline must reach a Kagi call that is already in flight; the row
// stays processing for RecoverIncomplete.
func TestDrainDeadlineCancelsAKagiCallInFlight(t *testing.T) {
	asked := make(chan struct{})
	release := make(chan struct{})
	f := newWorkerFixture(t, kagiSettings, func(_ http.ResponseWriter, _ *http.Request) {
		close(asked)
		<-release
	})
	defer close(release)
	job := f.addEntry(t, "slow")
	f.queue(t, job)
	ctx, cancel := context.WithCancel(context.Background())
	finished := make(chan struct{})
	go func() {
		f.worker.process(ctx, job)
		close(finished)
	}()
	<-asked
	cancel()
	select {
	case <-finished:
	case <-time.After(5 * time.Second):
		t.Fatal("the job kept running after the drain deadline")
	}
	if row := f.row(t, job); row.Status != models.SummaryProcessing {
		t.Errorf("row = %+v, want it left processing", row)
	}
}
