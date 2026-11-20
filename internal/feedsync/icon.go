package feedsync

import (
	"context"
	"log/slog"
	"time"

	"github.com/henry40408/rdrs/internal/feedparse"
	"github.com/henry40408/rdrs/internal/models"
)

// iconMaxAgeDays is how long a stored feed icon is kept before a refetch.
const iconMaxAgeDays = 7

// iconBudget bounds one feed's whole icon chain (up to five requests).
const iconBudget = 30 * time.Second

// IconCandidates are where a feed's icon may be found.
type IconCandidates struct {
	IconURL   *string
	LogoURL   *string
	SiteURL   *string
	UserAgent string
}

// FetchedIcon is an icon ready to store.
type FetchedIcon struct {
	Data        []byte
	ContentType string
	SourceURL   string
}

// IconFetcher finds a feed's icon. A nil icon with a nil error means none exists.
type IconFetcher func(ctx context.Context, candidates IconCandidates) (*FetchedIcon, error)

// refreshIconWithinBudget runs refreshIcon under its own timeout and only logs
// a failure, because the entries are already stored.
func (s *Syncer) refreshIconWithinBudget(ctx context.Context, feed *models.Feed, parsed *feedparse.Feed) {
	budget := s.IconTimeout
	if budget <= 0 {
		budget = iconBudget
	}
	ctx, cancel := context.WithTimeout(ctx, budget)
	defer cancel()
	if err := s.refreshIcon(ctx, feed, parsed); err != nil {
		slog.Warn("feed icon refresh failed", "event", "feed.icon_refresh_failed", "feed_id", feed.ID, "error", err)
	}
}

// refreshIcon stores the feed icon when it is older than a week. Only the
// freshness check can fail the sync; fetch and save failures are logged.
func (s *Syncer) refreshIcon(ctx context.Context, feed *models.Feed, parsed *feedparse.Feed) error {
	if s.FetchIcon == nil {
		return nil
	}
	if s.iconBreaker.isOpen(feed.ID, s.now()) {
		slog.Debug("feed icon refresh skipped after earlier failures", "event", "feed.icon_skipped", "feed_id", feed.ID)
		return nil
	}
	stale, err := models.ImageNeedsRefresh(ctx, s.DB, models.ImageRefreshParams{
		EntityType: models.EntityFeed, EntityID: feed.ID, MaxAgeDays: iconMaxAgeDays,
	})
	if err != nil || !stale {
		return err
	}
	icon, err := s.FetchIcon(ctx, IconCandidates{
		IconURL: parsed.Icon, LogoURL: parsed.Logo, SiteURL: feed.SiteURL,
		UserAgent: userAgentOf(feed, s.DefaultUserAgent),
	})
	if err != nil {
		slog.Warn("failed to fetch feed icon", "event", "feed.icon_fetch_failed", "feed_id", feed.ID, "error", err)
		s.iconBreaker.recordFailure(feed.ID, s.now())
		return nil
	}
	if icon == nil {
		slog.Debug("no icon found for feed", "event", "feed.icon_missing", "feed_id", feed.ID)
		s.iconBreaker.recordFailure(feed.ID, s.now())
		return nil
	}
	s.iconBreaker.recordSuccess(feed.ID)
	s.saveIcon(ctx, feed.ID, icon)
	return nil
}

func (s *Syncer) now() time.Time {
	if s.Now == nil {
		return time.Now()
	}
	return s.Now()
}

func (s *Syncer) saveIcon(ctx context.Context, feedID int64, icon *FetchedIcon) {
	err := models.UpsertImage(ctx, s.DB, models.ImageUpsert{
		EntityType: models.EntityFeed, EntityID: feedID, Data: icon.Data,
		ContentType: icon.ContentType, SourceURL: &icon.SourceURL,
	})
	if err != nil {
		slog.Warn("failed to save feed icon", "event", "feed.icon_save_failed", "feed_id", feedID, "error", err)
		return
	}
	slog.Debug("saved feed icon", "event", "feed.icon_saved", "feed_id", feedID, "url", icon.SourceURL)
}
