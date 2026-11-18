package models

import (
	"context"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

// EntityFeed is the image.entity_type of a feed icon.
const EntityFeed = "feed"

// Image is a stored blob (a feed icon) keyed by entity.
type Image struct {
	ID          int64
	EntityType  string
	EntityID    int64
	Data        []byte
	ContentType string
	SourceURL   *string
	FetchedAt   time.Time
	CreatedAt   time.Time
}

// FindImage looks an image up by entity.
func FindImage(ctx context.Context, e db.Executor, entityType string, entityID int64) (*Image, error) {
	row := e.QueryRowContext(ctx,
		`SELECT id, entity_type, entity_id, data, content_type, source_url, fetched_at, created_at
		 FROM image WHERE entity_type = $1 AND entity_id = $2`, entityType, entityID)
	var img Image
	err := row.Scan(&img.ID, &img.EntityType, &img.EntityID, &img.Data, &img.ContentType, &img.SourceURL,
		db.TimeDest(&img.FetchedAt), db.TimeDest(&img.CreatedAt))
	return orNil(&img, err)
}

// ImageUpsert is the input to UpsertImage.
type ImageUpsert struct {
	EntityType  string
	EntityID    int64
	Data        []byte
	ContentType string
	SourceURL   *string
}

// UpsertImage inserts or replaces the image for an entity.
func UpsertImage(ctx context.Context, e db.Executor, in ImageUpsert) error {
	_, err := e.ExecContext(ctx,
		`INSERT INTO image (entity_type, entity_id, data, content_type, source_url, fetched_at)
		 VALUES ($1, $2, $3, $4, $5, $6)
		 ON CONFLICT(entity_type, entity_id) DO UPDATE SET
		     data = excluded.data,
		     content_type = excluded.content_type,
		     source_url = excluded.source_url,
		     fetched_at = $6`,
		in.EntityType, in.EntityID, in.Data, in.ContentType, in.SourceURL, stamp(time.Now()))
	return err
}

// ImageExists reports whether an entity has an image.
func ImageExists(ctx context.Context, e db.Executor, entityType string, entityID int64) (bool, error) {
	count, err := countScalar(ctx, e,
		"SELECT COUNT(*) FROM image WHERE entity_type = $1 AND entity_id = $2", entityType, entityID)
	return count > 0, err
}

// ExistingImageIDs returns the subset of entityIDs that have an image of
// entityType, in one query, avoiding per-row ImageExists calls in list views.
func ExistingImageIDs(ctx context.Context, e db.Executor, entityType string, entityIDs []int64) (map[int64]bool, error) {
	existing := map[int64]bool{}
	if len(entityIDs) == 0 {
		return existing, nil
	}
	rows, err := e.QueryContext(ctx, "SELECT entity_id FROM image WHERE entity_type = $1", entityType)
	all, err := collect(rows, err, func(row scanner) (int64, error) {
		var id int64
		return id, row.Scan(&id)
	})
	if err != nil {
		return nil, err
	}
	have := make(map[int64]bool, len(all))
	for _, id := range all {
		have[id] = true
	}
	for _, id := range entityIDs {
		if have[id] {
			existing[id] = true
		}
	}
	return existing, nil
}

// ImageRefreshParams names the entity and the age limit of ImageNeedsRefresh.
type ImageRefreshParams struct {
	EntityType string
	EntityID   int64
	MaxAgeDays int64
}

// ImageNeedsRefresh reports whether an entity has no image fetched within
// maxAgeDays. The cutoff is computed here to avoid dialect-specific intervals.
func ImageNeedsRefresh(ctx context.Context, e db.Executor, p ImageRefreshParams) (bool, error) {
	cutoff := time.Now().Add(-time.Duration(p.MaxAgeDays) * 24 * time.Hour)
	count, err := countScalar(ctx, e,
		`SELECT COUNT(*) FROM image WHERE entity_type = $1 AND entity_id = $2 AND fetched_at > $3`,
		p.EntityType, p.EntityID, stamp(cutoff))
	return count == 0, err
}

// DeleteImageByEntity removes an entity's image.
func DeleteImageByEntity(ctx context.Context, e db.Executor, entityType string, entityID int64) error {
	_, err := e.ExecContext(ctx, "DELETE FROM image WHERE entity_type = $1 AND entity_id = $2", entityType, entityID)
	return err
}

// DeleteOrphanedFeedImages deletes up to batchSize feed images whose feed no
// longer exists and returns the count. image is polymorphic and has no foreign
// key, so removing a category or user (which cascades to feeds) and an icon
// fetch that finishes after its feed was deleted both leave orphans behind.
func DeleteOrphanedFeedImages(ctx context.Context, e db.Executor, batchSize int64) (int64, error) {
	return rowsAffected(e.ExecContext(ctx,
		`DELETE FROM image WHERE id IN (
		     SELECT id FROM image
		     WHERE entity_type = $1
		       AND NOT EXISTS (SELECT 1 FROM feed WHERE feed.id = image.entity_id)
		     LIMIT $2)`,
		EntityFeed, batchSize))
}
