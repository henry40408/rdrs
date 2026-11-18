package server

import (
	"net/http"
	"strings"

	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/search"
	"github.com/henry40408/rdrs/internal/views"
)

// searchPageSize is fixed rather than the entries-per-page setting: reading
// that costs a query on every keystroke of search-as-you-type.
const searchPageSize = 50

const snippetRunes = 200

func (a *App) searchRoutes() []Route {
	return []Route{{"GET", "/search", a.searchPage}}
}

// searchRequest is the query string of /search; unknown keys are ignored.
type searchRequest struct {
	q        string
	after    *string
	fragment *uint8
}

func parseSearchRequest(w http.ResponseWriter, r *http.Request) (searchRequest, bool) {
	reader := queryReader{values: r.URL.Query()}
	q, after, fragment := reader.text("q"), reader.text("after"), reader.small("fragment")
	if reader.err != nil {
		deserializeFailure(w, reader.err.Error())
		return searchRequest{}, false
	}
	request := searchRequest{after: after, fragment: fragment}
	if q != nil {
		request.q = strings.TrimSpace(*q)
	}
	return request, true
}

// searchPage is GET /search: the newest matches a page at a time; a blank q
// shows the form. ?fragment=1 answers live search (the whole results) or Load
// More (with after) with a fragment, except to a document navigation.
func (a *App) searchPage(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return
	}
	request, ok := parseSearchRequest(w, r)
	if !ok {
		return
	}
	results := a.searchResults(r, principal.User.ID, request)
	if request.fragment != nil && *request.fragment == 1 && !isDocumentNavigation(r) {
		if request.after != nil {
			writeHTML(w, http.StatusOK, views.SearchMore(results))
			return
		}
		writeHTML(w, http.StatusOK, views.SearchRefresh(results))
		return
	}
	layout := a.buildLayout(r, principal, flash.Take(w, r))
	writeHTML(w, http.StatusOK, views.Search(views.SearchPage{Layout: &layout, Results: results}))
}

func (a *App) searchResults(r *http.Request, userID int64, request searchRequest) views.SearchResults {
	results := views.SearchResults{Query: request.q}
	if request.q == "" {
		return results
	}
	node, err := search.Parse(request.q)
	if err != nil {
		message := searchErrorMessage(request.q, err)
		results.Error = &message
		return results
	}
	pageRequest := pageRequest{
		userID: userID, filter: entry.Filter{Query: node}, sort: entry.SortPublishedAt, pageSize: searchPageSize,
	}
	if request.after != nil && *request.after != "" {
		pageRequest.cursor, _ = entry.ParseCursor(*request.after)
	}
	// A failing query shows no matches rather than an error page.
	rows, next, err := a.fetchPage(r.Context(), pageRequest)
	if err != nil {
		return results
	}
	results.NextCursor = next
	terms := search.FreeTextTerms(node)
	for i := range rows {
		results.Results = append(results.Results, a.searchResultOf(&rows[i], terms))
	}
	return results
}

func (a *App) searchResultOf(found *entry.WithFeed, terms []string) views.SearchResult {
	feedTitle := found.FeedURL
	if found.FeedTitle != nil {
		feedTitle = *found.FeedTitle
	}
	// Unlike the list rows, the title is not entity-decoded here.
	title := untitledEntry
	if found.Title != nil {
		title = *found.Title
	}
	body := found.Content
	if body == nil {
		body = found.Summary
	}
	relative, iso := relativeTime(found.PublishedAt, a.Now())
	return views.SearchResult{
		EntryID: found.ID, TitleHTML: highlightHTML(title, terms), FeedTitle: feedTitle,
		PublishedRelative: relative, PublishedISO: iso,
		SnippetHTML: highlightHTML(buildSnippet(body, terms, snippetRunes), terms),
	}
}
