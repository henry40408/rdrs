package server

import (
	"context"
	"time"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/pixel"
	"github.com/henry40408/rdrs/internal/sanitize"
	"github.com/henry40408/rdrs/internal/save"
	"github.com/henry40408/rdrs/internal/textutil"
	"github.com/henry40408/rdrs/internal/views"
)

// paneSettings is what the reading pane needs from the user's settings.
type paneSettings struct {
	// hasSave shows the Save button; hasKagi the Summarize button.
	hasSave, hasKagi bool
	pixelEnabledAt   *time.Time
}

// loadPaneSettings reads the pane settings in one query. An unreadable
// credential shows as absent; the settings page explains why.
func (a *App) loadPaneSettings(ctx context.Context, userID int64) (paneSettings, error) {
	settings, err := models.LoadUserSettings(ctx, a.DB, userID)
	if err != nil {
		return paneSettings{}, DatabaseError(err)
	}
	key, _ := a.Config.ServiceTokenKey()
	services := save.ConfigOf(settings.SaveServicesJSON(key))
	return paneSettings{
		hasSave:        services.HasAnyService(),
		hasKagi:        services.Kagi != nil && services.Kagi.IsConfigured(),
		pixelEnabledAt: settings.PixelTrackingSince,
	}, nil
}

// contentView says which body the pane renders; the two are identical until
// an article has been fetched.
type contentView int

const (
	// fullView is the stored fetched article, if any.
	fullView contentView = iota
	// originalView is what the feed published (?view=original).
	originalView
)

// renderPurpose says who a render is for. A speculative copy must not carry
// the open-tracking pixel, or it would report an open nobody performed.
type renderPurpose int

const (
	readerRender renderPurpose = iota
	// speculativeRender is an ?offline=1 mirror or a prefetch.
	speculativeRender
)

// paneRequest is what buildReadingPane renders.
type paneRequest struct {
	userID   int64
	entry    *entry.WithFeed
	settings paneSettings
	view     contentView
	purpose  renderPurpose
}

// builtPane is a pane and the summary status its list row shows.
type builtPane struct {
	pane          *views.ReadingPane
	summaryStatus models.SummaryStatus
}

// buildReadingPane sanitises the body, injects the open pixel and resolves the
// summary of an already-loaded entry.
func (a *App) buildReadingPane(ctx context.Context, request paneRequest) (builtPane, error) {
	e := request.entry
	stored := ""
	if e.FullContent != nil {
		stored = *e.FullContent
	}
	showingFull := request.view == fullView && stored != ""
	content := a.sanitizedBody(request, showingFull, stored)
	summary, err := a.resolveSummary(ctx, request.userID, e.ID)
	if err != nil {
		return builtPane{}, err
	}
	pane := &views.ReadingPane{
		ID: e.ID, Title: entryTitle(e.Title), Link: cleanLink(e.Link), FeedID: e.FeedID, FeedHasIcon: e.FeedHasIcon,
		ContentHTML: content, IsRead: e.ReadAt != nil, IsStarred: e.StarredAt != nil,
		SummaryText: summary.text, SummaryInFlight: summary.inFlight, SummaryError: summary.failure,
		HasKagi: request.settings.hasKagi, HasSave: request.settings.hasSave,
		IsFullContent: showingFull, HasStoredFull: stored != "",
	}
	if e.FeedTitle != nil {
		pane.FeedTitle = *e.FeedTitle
	}
	if e.Author != nil {
		decoded := textutil.DecodeHTMLEntities(*e.Author)
		pane.Author = &decoded
	}
	pane.PublishedRelative, _ = relativeTime(e.PublishedAt, a.Now())
	if e.PublishedAt != nil {
		iso := rfc3339Offset(*e.PublishedAt)
		pane.PublishedISO = &iso
	}
	return builtPane{pane: pane, summaryStatus: summary.status}, nil
}

// sanitizedBody is the pane's HTML. The pixel goes in after sanitising, which
// would strip or proxy it, and is skipped for speculative renders, whose images
// offline.js may walk.
func (a *App) sanitizedBody(request paneRequest, showingFull bool, stored string) string {
	e := request.entry
	raw := stored
	if !showingFull {
		raw = firstText(e.Content, e.Summary)
	}
	sanitizeContext := sanitize.Context{
		Secret: a.Config.Secret, BaseURL: e.ContentBaseURL(), ProxyBaseURL: a.Config.PublicBaseURL,
	}
	if e.CustomReferrer != nil {
		sanitizeContext.Referrer = *e.CustomReferrer
	}
	content := sanitize.HTML(raw, sanitizeContext)
	tracker := pixel.Disabled(request.userID, a.Config.Secret)
	if request.purpose == readerRender {
		tracker.EnabledAt = request.settings.pixelEnabledAt
	}
	// Same-origin page, so the pixel URL stays root-relative.
	return tracker.MaybeInject(content, e.ID, e.CreatedAt)
}

// firstText is the first non-nil value, or "".
func firstText(values ...*string) string {
	for _, value := range values {
		if value != nil {
			return *value
		}
	}
	return ""
}

// resolvedSummary is a pane's summary. The text has been through the summary
// sanitiser, like feed content.
type resolvedSummary struct {
	text     *string
	inFlight bool
	failure  *string
	// status is empty when the entry has no summary.
	status models.SummaryStatus
}

func summaryOf(status models.SummaryStatus, text, failure *string) resolvedSummary {
	summary := resolvedSummary{
		inFlight: status == models.SummaryPending || status == models.SummaryProcessing,
		status:   status,
	}
	if status == models.SummaryCompleted && text != nil {
		cleaned := sanitize.Summary(*text)
		summary.text = &cleaned
	}
	if status == models.SummaryFailed {
		summary.failure = failure
	}
	return summary
}

// resolveSummary reads the cache first, then the entry_summary table.
func (a *App) resolveSummary(ctx context.Context, userID, entryID int64) (resolvedSummary, error) {
	// A cached failure falls through: a retry may have updated the row.
	if cached, ok := a.Summaries.Get(userID, entryID); ok && cached.Status != models.SummaryFailed {
		return summaryOf(cached.Status, cached.SummaryText, nil), nil
	}
	stored, err := models.FindEntrySummary(ctx, a.DB, userID, entryID)
	if err != nil {
		return resolvedSummary{}, DatabaseError(err)
	}
	if stored == nil {
		return resolvedSummary{}, nil
	}
	return summaryOf(stored.Status, stored.SummaryText, stored.ErrorMessage), nil
}
