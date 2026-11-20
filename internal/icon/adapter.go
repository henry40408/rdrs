package icon

import (
	"context"

	"github.com/henry40408/rdrs/internal/feedsync"
	"github.com/henry40408/rdrs/internal/fetch"
)

// ForProduction is the icon hook the syncer runs with: the same SSRF-guarded
// fetcher as feed fetches, with the shorter icon retry.
func ForProduction(fetcher *fetch.Fetcher) feedsync.IconFetcher {
	return Fetcher{Fetcher: fetcher, Retry: fetch.IconRetry()}.ForSync()
}

// ForSync adapts the fetcher to the hook feedsync calls. The user agent is
// per feed, so it comes with each call. The result is never an error: a
// missing icon is not a failure.
func (f Fetcher) ForSync() feedsync.IconFetcher {
	return func(ctx context.Context, c feedsync.IconCandidates) (*feedsync.FetchedIcon, error) {
		withAgent := f
		withAgent.UserAgent = c.UserAgent
		img := withAgent.Fetch(ctx, Candidates{IconURL: c.IconURL, LogoURL: c.LogoURL, SiteURL: c.SiteURL})
		if img == nil {
			return nil, nil
		}
		return &feedsync.FetchedIcon{Data: img.Data, ContentType: img.ContentType, SourceURL: img.SourceURL}, nil
	}
}
