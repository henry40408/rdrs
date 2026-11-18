package harness

import (
	"net/http"
	"net/http/httptest"
	"time"
)

// mock is an upstream the server under test talks to.
type mock = httptest.Server

// mockFeedBody is what the mock upstream serves for every feed URL a scenario
// subscribes to.
const mockFeedBody = `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Test Feed</title>
    <link>http://localhost</link>
    <description>A test feed for E2E tests</description>
  </channel>
</rss>`

// kagiLatency delays the mock Kagi: summarizer scenarios assert on pending
// and processing SSE events an instant response would race past.
const kagiLatency = 300 * time.Millisecond

func startFeedMock() *mock {
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/rss+xml")
		_, _ = w.Write([]byte(mockFeedBody))
	}))
}

// startKagiMock stands in for the Kagi summarisation API.
func startKagiMock() *mock {
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		time.Sleep(kagiLatency)
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"output_data":{"markdown":"E2E mock summary body."}}`))
	}))
}
