package server

import (
	"bufio"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/models"
)

// openStream connects /events with the admin's session; it returns the reader
// and the response.
func openStream(t *testing.T, f *signedInFixture) (*bufio.Reader, *http.Response) {
	t.Helper()
	server := httptest.NewServer(f.handler)
	t.Cleanup(server.Close)
	request, err := http.NewRequest(http.MethodGet, server.URL+"/events", nil)
	if err != nil {
		t.Fatal(err)
	}
	request.AddCookie(f.session)
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = response.Body.Close() })
	return bufio.NewReader(response.Body), response
}

func readBlock(t *testing.T, reader *bufio.Reader) string {
	t.Helper()
	done := make(chan string, 1)
	go func() {
		var block strings.Builder
		for {
			line, err := reader.ReadString('\n')
			block.WriteString(line)
			if err != nil || line == "\n" {
				done <- block.String()
				return
			}
		}
	}()
	select {
	case block := <-done:
		return block
	case <-time.After(3 * time.Second):
		t.Fatal("no event within 3s")
		return ""
	}
}

func TestEventsStreamsOnlyTheSignedInUsersEvents(t *testing.T) {
	f := newSignedIn(t)
	reader, response := openStream(t, f)
	if got := response.Header.Get("Content-Type"); got != "text/event-stream" {
		t.Fatalf("Content-Type = %q", got)
	}
	if response.Header.Get("Content-Encoding") != "" || response.Header.Get("Etag") != "" {
		t.Error("the stream went through Compress or ETag")
	}
	status := models.SummaryCompleted
	f.app.Events.EmitSidebar(f.adminID(t) + 1)
	f.app.Events.EmitSummary(f.adminID(t), 50, &status)
	if got := readBlock(t, reader); got != "event: summary\ndata: {\"entry_id\":50,\"status\":\"completed\"}\n\n" {
		t.Fatalf("first event = %q", got)
	}
	f.app.Events.EmitSidebar(f.adminID(t))
	if got := readBlock(t, reader); got != "event: sidebar\ndata: 1\n\n" {
		t.Fatalf("sidebar event = %q", got)
	}
}

func TestEventsStreamEndsWhenTheBusCloses(t *testing.T) {
	f := newSignedIn(t)
	reader, _ := openStream(t, f)
	f.app.Events.Close()
	done := make(chan error, 1)
	go func() { _, err := reader.ReadByte(); done <- err }()
	select {
	case err := <-done:
		if err == nil {
			t.Fatal("received data instead of the end of the stream")
		}
	case <-time.After(3 * time.Second):
		t.Fatal("the stream outlived the bus")
	}
}

func TestEventsRefusesAnAnonymousVisitor(t *testing.T) {
	f := newAuthFixture(t)
	rec := f.do(httptest.NewRequest(http.MethodGet, "/events", nil))
	if rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != "/login" {
		t.Fatalf("= %d %q", rec.Code, rec.Header().Get("Location"))
	}
}
