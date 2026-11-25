package siphash

import (
	"testing"
)

var rustBucketVectors = []struct {
	url    string
	bucket uint64
}{
	{"", 19},
	{"https://example.com/feed.xml", 17},
	{"https://example.org/rss", 42},
	{"http://blog.example.net/atom?x=1", 6},
	{"https://日本語.example/フィード", 45},
}

func TestFeedBucketMatchesRust(t *testing.T) {
	for _, row := range rustBucketVectors {
		if got := RustStrHash(row.url) % 60; got != row.bucket {
			t.Errorf("%q: bucket %d, Rust chose %d", row.url, got, row.bucket)
		}
	}
}
