package cache

import (
	"testing"
	"time"
)

// Port of tests/page_cache_test.rs: the contract page handlers rely on.

type pageKey struct {
	userID int64
	page   string
}

func TestInsertAndGetRoundtrip(t *testing.T) {
	cache := NewTTL[pageKey, string](64, time.Minute)
	cache.Put(pageKey{1, "sidebar"}, "payload-A")

	if got, ok := cache.Get(pageKey{1, "sidebar"}); !ok || got != "payload-A" {
		t.Fatalf("got %q, %v", got, ok)
	}
	if _, ok := cache.Get(pageKey{1, "feeds"}); ok {
		t.Error("another page hit")
	}
	if _, ok := cache.Get(pageKey{2, "sidebar"}); ok {
		t.Error("another user hit")
	}
}

func TestInvalidateRemovesEntry(t *testing.T) {
	cache := NewTTL[pageKey, string](64, time.Minute)
	cache.Put(pageKey{1, "sidebar"}, "payload-A")
	cache.Delete(pageKey{1, "sidebar"})

	if _, ok := cache.Get(pageKey{1, "sidebar"}); ok {
		t.Fatal("entry survived Delete")
	}
}

func TestTtlExpiryDropsEntry(t *testing.T) {
	cache := NewTTL[pageKey, string](64, 50*time.Millisecond)
	cache.Put(pageKey{1, "sidebar"}, "payload-A")
	if _, ok := cache.Get(pageKey{1, "sidebar"}); !ok {
		t.Fatal("entry missing before expiry")
	}

	time.Sleep(120 * time.Millisecond)

	if _, ok := cache.Get(pageKey{1, "sidebar"}); ok {
		t.Fatal("entry survived its TTL")
	}
}
