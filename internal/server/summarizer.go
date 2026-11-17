package server

import (
	"context"
	"fmt"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"

	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/sanitize"
	"github.com/henry40408/rdrs/internal/save"
	"github.com/henry40408/rdrs/internal/summarize"
	"github.com/henry40408/rdrs/internal/views"
)

func (a *App) summarizerRoutes() []Route {
	return []Route{
		{"GET", "/summarizer", a.summarizerPage},
		{"POST", "/summarizer", a.summarizerStart},
		{"POST", "/summarizer/item", a.summarizerItem},
	}
}

// maxSummarizerURLs is the most links one run accepts.
const maxSummarizerURLs = 30

// inflightUsers holds the users with a /summarizer/item request running. It
// allows one live Kagi call per user, so a burst of POSTs cannot fan out.
type inflightUsers struct {
	mu    sync.Mutex
	users map[int64]struct{}
}

// begin reserves the user's slot and returns its release, or false when a call
// is already running.
func (s *inflightUsers) begin(userID int64) (release func(), ok bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, busy := s.users[userID]; busy {
		return nil, false
	}
	if s.users == nil {
		s.users = make(map[int64]struct{})
	}
	s.users[userID] = struct{}{}
	return func() {
		s.mu.Lock()
		defer s.mu.Unlock()
		delete(s.users, userID)
	}, true
}

// parseHTTPURL is url.Parse held to what Rust's Url::parse accepted: a scheme
// is required, and an http(s) URL needs a host.
func parseHTTPURL(raw string) (*url.URL, bool) {
	parsed, err := url.Parse(raw)
	if err != nil || parsed.Scheme == "" {
		return nil, false
	}
	if (parsed.Scheme == "http" || parsed.Scheme == "https") && parsed.Host == "" {
		return nil, false
	}
	return parsed, true
}

// parseURLLines turns the textarea into a de-duplicated, ordered list of
// 1..maxSummarizerURLs links that passed the SSRF guard. The error is the
// text shown to the user.
func parseURLLines(input string) ([]string, error) {
	seen := make(map[string]bool)
	var urls []string
	for _, raw := range strings.Split(input, "\n") {
		line := strings.TrimSpace(raw)
		if line == "" {
			continue
		}
		parsed, ok := parseHTTPURL(line)
		if !ok {
			return nil, fmt.Errorf("Not a valid URL: %s", line)
		}
		if parsed.Scheme != "http" && parsed.Scheme != "https" {
			return nil, fmt.Errorf("Only http(s) URLs are supported: %s", line)
		}
		if err := fetch.ValidateURL(parsed); err != nil {
			return nil, fmt.Errorf("URL not allowed (%s): %w", line, err)
		}
		if !seen[line] {
			seen[line] = true
			urls = append(urls, line)
		}
	}
	if len(urls) == 0 {
		return nil, fmt.Errorf("Enter at least one URL.")
	}
	if len(urls) > maxSummarizerURLs {
		return nil, fmt.Errorf("Too many URLs — %d max per run.", maxSummarizerURLs)
	}
	return urls, nil
}

// urlHost is the card-title fallback; an unparseable input is returned as is.
func urlHost(raw string) string {
	if parsed, ok := parseHTTPURL(raw); ok && parsed.Hostname() != "" {
		return parsed.Hostname()
	}
	return raw
}

// kagiSetting is the user's Kagi config; an unreadable setting counts as unset.
func (a *App) kagiSetting(ctx context.Context, userID int64) (save.KagiConfig, bool) {
	services, err := a.usableServices(ctx, userID)
	if err != nil || services.Kagi == nil || !services.Kagi.IsConfigured() {
		return save.KagiConfig{}, false
	}
	return *services.Kagi, true
}

func (a *App) summarizerPage(w http.ResponseWriter, r *http.Request) {
	a.serveSummarizer(w, r, "", nil)
}

func (a *App) summarizerStart(w http.ResponseWriter, r *http.Request) {
	form, ok := parseForm(w, r)
	if !ok || !requireFields(w, form, "urls") {
		return
	}
	a.serveSummarizer(w, r, form.Get("urls"), form)
}

// serveSummarizer renders the page: empty for a GET, with queued cards or an
// error for a submitted form (non-nil form).
func (a *App) serveSummarizer(w http.ResponseWriter, r *http.Request, urlsText string, form url.Values) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return
	}
	layout := a.buildLayout(r, principal, flash.Take(w, r))
	_, configured := a.kagiSetting(r.Context(), principal.User.ID)
	page := views.SummarizerPage{Layout: &layout, KagiConfigured: configured, URLsText: urlsText, CSRFToken: principal.CSRFToken}
	if form != nil {
		page.Cards, page.Error = queuedCards(urlsText)
	}
	writeHTML(w, http.StatusOK, views.Summarizer(page))
}

func queuedCards(urlsText string) ([]views.SummarizerCard, *string) {
	urls, err := parseURLLines(urlsText)
	if err != nil {
		message := err.Error()
		return nil, &message
	}
	cards := make([]views.SummarizerCard, len(urls))
	for i, link := range urls {
		cards[i] = views.SummarizerCard{Index: i, URL: link, Title: urlHost(link), State: "queued"}
	}
	return cards, nil
}

// itemForm is one card the page asks to summarize.
type itemForm struct {
	url   string
	index int
}

func parseItemForm(w http.ResponseWriter, r *http.Request) (itemForm, bool) {
	form, ok := parseForm(w, r)
	if !ok || !requireFields(w, form, "url", "index") {
		return itemForm{}, false
	}
	index, err := strconv.ParseUint(form.Get("index"), 10, 63)
	if err != nil {
		rejection(w, http.StatusUnprocessableEntity,
			"Failed to deserialize form body: index: "+integerFailure(form.Get("index"), err))
		return itemForm{}, false
	}
	return itemForm{url: form.Get("url"), index: int(index)}, true
}

// summarizerItem is POST /summarizer/item: one Kagi call, answered with the
// card's final state.
func (a *App) summarizerItem(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return
	}
	form, ok := parseItemForm(w, r)
	if !ok {
		return
	}
	card := a.summarizeItem(r.Context(), principal.User.ID, form)
	writeHTML(w, http.StatusOK, views.SummarizerCardFragment(card))
}

func (a *App) summarizeItem(ctx context.Context, userID int64, form itemForm) views.SummarizerCard {
	host := urlHost(form.url)
	failed := func(message string) views.SummarizerCard {
		return views.SummarizerCard{Index: form.index, URL: form.url, Title: host, State: "error", Error: message}
	}
	// The browser could post anything, so the link is checked again.
	parsed, ok := parseHTTPURL(form.url)
	if !ok || (parsed.Scheme != "http" && parsed.Scheme != "https") {
		return failed("Not a valid URL.")
	}
	if fetch.ValidateURL(parsed) != nil {
		return failed("URL not allowed.")
	}
	release, ok := a.summarizerInflight.begin(userID)
	if !ok {
		return failed("Another summary is already in progress — wait for it to finish.")
	}
	defer release()
	config, ok := a.kagiSetting(ctx, userID)
	if !ok {
		return failed("Kagi is not configured.")
	}
	result, err := summarize.Summarize(ctx, summarize.Request{
		Config: config, URL: form.url, Client: a.externalClient, Retry: a.requestRetry,
	})
	if err != nil {
		return failed(err.Error())
	}
	if !result.Success {
		if result.Error != nil {
			return failed(*result.Error)
		}
		return failed("Summarization failed.")
	}
	card := views.SummarizerCard{Index: form.index, URL: form.url, Title: host, State: "completed"}
	if result.Title != nil {
		card.Title = *result.Title
	}
	if result.OutputText != nil {
		card.Summary = sanitize.Summary(*result.OutputText)
	}
	return card
}
