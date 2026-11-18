package siphash

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestFeedBucketMatchesRust(t *testing.T) {
	raw, err := os.ReadFile(filepath.Join("..", "..", "testdata", "vectors", "feed_bucket.json"))
	if err != nil {
		t.Fatal(err)
	}
	var rows []struct {
		URL    string `json:"url"`
		Bucket uint64 `json:"bucket"`
	}
	if err := json.Unmarshal(raw, &rows); err != nil {
		t.Fatal(err)
	}
	if len(rows) == 0 {
		t.Fatal("no vectors")
	}
	for _, row := range rows {
		if got := RustStrHash(row.URL) % 60; got != row.Bucket {
			t.Errorf("%q: bucket %d, Rust chose %d", row.URL, got, row.Bucket)
		}
	}
}
