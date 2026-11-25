package models

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

func TestCategoryLifecycle(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		alice, bob := mustUser(t, database, "alice"), mustUser(t, database, "bob")
		news := mustCategory(t, database, alice.ID, "News")
		mustCategory(t, database, alice.ID, "Blogs")
		mustCategory(t, database, bob.ID, "News") // same name, different user

		if _, err := CreateCategory(ctx, database, alice.ID, "News"); !errors.Is(err, ErrCategoryExists) {
			t.Errorf("duplicate err = %v", err)
		}
		list, err := ListCategoriesByUser(ctx, database, alice.ID)
		if err != nil || len(list) != 2 || list[0].Name != "Blogs" {
			t.Errorf("list = %+v, %v", list, err)
		}
		if found, _ := FindCategoryByIDAndUser(ctx, database, news.ID, bob.ID); found != nil {
			t.Error("ownership check failed")
		}
		if found, _ := FindCategoryByNameAndUser(ctx, database, "News", alice.ID); found == nil || found.ID != news.ID {
			t.Errorf("by name = %+v", found)
		}
		renamed, err := RenameCategory(ctx, database, news.ID, alice.ID, "World")
		if err != nil || renamed.Name != "World" {
			t.Fatalf("rename = %+v, %v", renamed, err)
		}
		if _, err := RenameCategory(ctx, database, news.ID, alice.ID, "Blogs"); !errors.Is(err, ErrCategoryExists) {
			t.Errorf("rename conflict err = %v", err)
		}
		if _, err := RenameCategory(ctx, database, news.ID, bob.ID, "X"); !errors.Is(err, ErrCategoryNotFound) {
			t.Errorf("foreign rename err = %v", err)
		}
		if err := DeleteCategory(ctx, database, news.ID, alice.ID); err != nil {
			t.Fatal(err)
		}
		if err := DeleteCategory(ctx, database, news.ID, alice.ID); !errors.Is(err, ErrCategoryNotFound) {
			t.Errorf("second delete err = %v", err)
		}
	})
}

func TestFeedCreateFindAndBucket(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		category := mustCategory(t, database, user.ID, "News")
		feed, err := CreateFeed(ctx, database, CreateFeedParams{CategoryID: category.ID, URL: "https://example.com/feed.xml", Title: ptr("Example")})
		if err != nil {
			t.Fatal(err)
		}
		if feed.Bucket == nil || *feed.Bucket != URLToBucket(feed.URL) || *feed.Bucket < 0 || *feed.Bucket >= 60 {
			t.Errorf("bucket = %v", feed.Bucket)
		}
		if feed.Title == nil || *feed.Title != "Example" || feed.HTTP2Disabled || feed.FetchedAt != nil {
			t.Errorf("feed = %+v", feed)
		}
		if _, err := CreateFeed(ctx, database, CreateFeedParams{CategoryID: category.ID, URL: feed.URL}); !errors.Is(err, ErrFeedExists) {
			t.Errorf("duplicate err = %v", err)
		}
		other := mustCategory(t, database, user.ID, "Other")
		mustFeed(t, database, other.ID, feed.URL) // same URL, different category

		byURL, _ := FindFeedByURLAndCategory(ctx, database, feed.URL, category.ID)
		forUser, _ := FindFeedByURLForUser(ctx, database, feed.URL, user.ID)
		if byURL == nil || byURL.ID != feed.ID || forUser == nil {
			t.Errorf("lookups: %+v %+v", byURL, forUser)
		}
		if n, _ := CountFeedsByUser(ctx, database, user.ID); n != 2 {
			t.Errorf("count = %d", n)
		}
		byBucket, err := ListFeedsByBucket(ctx, database, *feed.Bucket)
		if err != nil || len(byBucket) < 1 {
			t.Errorf("by bucket = %+v, %v", byBucket, err)
		}
	})
}

var rustBucketVectors = []struct {
	url    string
	bucket int64
}{
	{"", 19},
	{"https://example.com/feed.xml", 17},
	{"https://example.org/rss", 42},
	{"http://blog.example.net/atom?x=1", 6},
	{"https://日本語.example/フィード", 45},
}

func TestURLToBucketMatchesRustDefaultHasher(t *testing.T) {
	for _, v := range rustBucketVectors {
		if got := URLToBucket(v.url); got != v.bucket {
			t.Errorf("URLToBucket(%q) = %d, want %d", v.url, got, v.bucket)
		}
	}
}

func TestFeedListingOrderAndOwners(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		alice, bob := mustUser(t, database, "alice"), mustUser(t, database, "bob")
		ca, cb := mustCategory(t, database, alice.ID, "A"), mustCategory(t, database, bob.ID, "B")
		second, _ := CreateFeed(ctx, database, CreateFeedParams{CategoryID: ca.ID, URL: "https://b.example", Title: ptr("Zed")})
		first, _ := CreateFeed(ctx, database, CreateFeedParams{CategoryID: ca.ID, URL: "https://a.example", Title: ptr("Alpha")})
		foreign := mustFeed(t, database, cb.ID, "https://c.example")

		list, err := ListFeedsByUser(ctx, database, alice.ID)
		if err != nil || len(list) != 2 || list[0].ID != first.ID || list[1].ID != second.ID {
			t.Errorf("by user = %+v, %v", list, err)
		}
		if byCat, _ := ListFeedsByCategory(ctx, database, ca.ID); len(byCat) != 2 {
			t.Errorf("by category = %+v", byCat)
		}
		owners, err := OwnerUserIDsForFeeds(ctx, database, []int64{first.ID, second.ID, foreign.ID})
		if err != nil || len(owners) != 2 {
			t.Errorf("owners = %v, %v", owners, err)
		}
		if none, err := OwnerUserIDsForFeeds(ctx, database, nil); err != nil || none != nil {
			t.Errorf("empty owners = %v, %v", none, err)
		}
	})
}

func TestUpdateFeedMovesAndRehashes(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		from, to := mustCategory(t, database, user.ID, "From"), mustCategory(t, database, user.ID, "To")
		feed := mustFeed(t, database, from.ID, "https://old.example")

		updated, err := UpdateFeed(ctx, database, UpdateFeedParams{
			ID: feed.ID, CategoryID: from.ID, NewCategoryID: to.ID, URL: "https://new.example",
			Title: ptr("New"), HTTP2Disabled: true, CustomReferrer: ptr("https://ref"),
		})
		if err != nil {
			t.Fatal(err)
		}
		if updated.CategoryID != to.ID || !updated.HTTP2Disabled || *updated.Bucket != URLToBucket("https://new.example") {
			t.Errorf("updated = %+v", updated)
		}
		if _, err := UpdateFeed(ctx, database, UpdateFeedParams{ID: feed.ID, CategoryID: from.ID, NewCategoryID: to.ID, URL: "x"}); !errors.Is(err, ErrFeedNotFound) {
			t.Errorf("stale category err = %v", err)
		}
		other := mustFeed(t, database, to.ID, "https://taken.example")
		_, err = UpdateFeed(ctx, database, UpdateFeedParams{ID: other.ID, CategoryID: to.ID, NewCategoryID: to.ID, URL: "https://new.example"})
		if !errors.Is(err, ErrFeedExists) {
			t.Errorf("conflict err = %v", err)
		}
	})
}

func TestUpdateFetchResultKeepsFeedUpdatedAtWhenNil(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		feed := mustFeed(t, database, mustCategory(t, database, user.ID, "News").ID, "https://example.com")
		published := time.Now().Add(-time.Hour)
		fetched := time.Now()

		err := UpdateFetchResult(ctx, database, FetchResult{ID: feed.ID, FetchedAt: fetched, ETag: ptr("etag"), FeedUpdatedAt: &published})
		if err != nil {
			t.Fatal(err)
		}
		err = UpdateFetchResult(ctx, database, FetchResult{ID: feed.ID, FetchedAt: fetched, FetchError: ptr("boom")})
		if err != nil {
			t.Fatal(err)
		}
		got, _ := FindFeedByID(ctx, database, feed.ID)
		if got.FeedUpdatedAt == nil || !sameInstant(*got.FeedUpdatedAt, published) {
			t.Errorf("feed_updated_at = %v, want %v", got.FeedUpdatedAt, published)
		}
		if got.FetchError == nil || *got.FetchError != "boom" || got.ETag != nil {
			t.Errorf("fetch fields = %+v", got)
		}
	})
}

func TestDeleteFeedRemovesIconAndCascades(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		category := mustCategory(t, database, user.ID, "News")
		feed := mustFeed(t, database, category.ID, "https://example.com")
		if err := UpsertImage(ctx, database, ImageUpsert{EntityType: EntityFeed, EntityID: feed.ID, Data: []byte{1}, ContentType: "image/png"}); err != nil {
			t.Fatal(err)
		}
		if err := DeleteFeed(ctx, database, feed.ID, category.ID); err != nil {
			t.Fatal(err)
		}
		if ok, _ := ImageExists(ctx, database, EntityFeed, feed.ID); ok {
			t.Error("icon survived its feed")
		}
		if err := DeleteFeed(ctx, database, feed.ID, category.ID); !errors.Is(err, ErrFeedNotFound) {
			t.Errorf("second delete err = %v", err)
		}
		kept := mustFeed(t, database, category.ID, "https://kept.example")
		if err := DeleteCategory(ctx, database, category.ID, user.ID); err != nil {
			t.Fatal(err)
		}
		if found, _ := FindFeedByID(ctx, database, kept.ID); found != nil {
			t.Error("feed survived its category")
		}
	})
}

func TestImageUpsertFindAndRefresh(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		if ok, _ := ImageExists(ctx, database, EntityFeed, 1); ok {
			t.Error("phantom image")
		}
		if needs, _ := ImageNeedsRefresh(ctx, database, ImageRefreshParams{EntityType: EntityFeed, EntityID: 1, MaxAgeDays: 7}); !needs {
			t.Error("a missing image needs refresh")
		}
		in := ImageUpsert{EntityType: EntityFeed, EntityID: 1, Data: []byte{1, 2, 3}, ContentType: "image/png", SourceURL: ptr("https://example.com/i.png")}
		if err := UpsertImage(ctx, database, in); err != nil {
			t.Fatal(err)
		}
		in.Data, in.ContentType, in.SourceURL = []byte{9}, "image/gif", nil
		if err := UpsertImage(ctx, database, in); err != nil {
			t.Fatal(err)
		}
		img, err := FindImage(ctx, database, EntityFeed, 1)
		if err != nil || img == nil || len(img.Data) != 1 || img.Data[0] != 9 || img.ContentType != "image/gif" || img.SourceURL != nil {
			t.Fatalf("image = %+v, %v", img, err)
		}
		if needs, _ := ImageNeedsRefresh(ctx, database, ImageRefreshParams{EntityType: EntityFeed, EntityID: 1, MaxAgeDays: 7}); needs {
			t.Error("a fresh image does not need refresh")
		}
		mustExec(t, database, "UPDATE image SET fetched_at = $1", stamp(time.Now().Add(-10*24*time.Hour)))
		if needs, _ := ImageNeedsRefresh(ctx, database, ImageRefreshParams{EntityType: EntityFeed, EntityID: 1, MaxAgeDays: 7}); !needs {
			t.Error("a stale image needs refresh")
		}
		existing, err := ExistingImageIDs(ctx, database, EntityFeed, []int64{1, 2, 3})
		if err != nil || len(existing) != 1 || !existing[1] {
			t.Errorf("existing = %v, %v", existing, err)
		}
		if err := DeleteImageByEntity(ctx, database, EntityFeed, 1); err != nil {
			t.Fatal(err)
		}
		if gone, _ := FindImage(ctx, database, EntityFeed, 1); gone != nil {
			t.Error("image not deleted")
		}
	})
}

func TestOrphanSweepRemovesOnlyImagesOfMissingFeeds(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		feed := mustFeed(t, database, mustCategory(t, database, user.ID, "News").ID, "https://example.com")
		for _, id := range []int64{feed.ID, 9001, 9002, 9003} {
			in := ImageUpsert{EntityType: EntityFeed, EntityID: id, Data: []byte{1}, ContentType: "image/png"}
			if err := UpsertImage(ctx, database, in); err != nil {
				t.Fatal(err)
			}
		}
		if n, err := DeleteOrphanedFeedImages(ctx, database, 2); err != nil || n != 2 {
			t.Fatalf("first batch = %d, %v; want 2", n, err)
		}
		if n, err := DeleteOrphanedFeedImages(ctx, database, 2); err != nil || n != 1 {
			t.Fatalf("second batch = %d, %v; want 1", n, err)
		}
		if ok, _ := ImageExists(ctx, database, EntityFeed, feed.ID); !ok {
			t.Error("a live feed's icon was swept")
		}
	})
}
