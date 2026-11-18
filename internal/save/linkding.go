package save

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"strconv"
	"strings"

	"github.com/henry40408/rdrs/internal/fetch"
)

// LinkdingConfig is the user's Linkding setting.
type LinkdingConfig struct {
	APIURL   string `json:"api_url"`
	APIToken string `json:"api_token"`
}

// IsConfigured needs both fields: a half-filled form is not configured.
func (c LinkdingConfig) IsConfigured() bool {
	return c.APIURL != "" && c.APIToken != ""
}

type linkdingBookmark struct {
	URL         string   `json:"url"`
	Title       *string  `json:"title,omitempty"`
	Description *string  `json:"description,omitempty"`
	TagNames    []string `json:"tag_names,omitempty"`
}

// LinkdingRequest is one save.
type LinkdingRequest struct {
	Config   LinkdingConfig
	Bookmark BookmarkData
	// Client is not SSRF guarded on purpose: Linkding is usually a private
	// host the user runs themselves.
	Client *http.Client
	Retry  fetch.RetryConfig
}

func linkdingResult(success bool, message string, bookmarkURL *string) *Result {
	return &Result{Success: success, Service: "linkding", Message: message, BookmarkURL: bookmarkURL}
}

// SaveToLinkding never fails the request for a Linkding-side problem: it comes
// back as Success false so one integration cannot fail the others. Only
// transport and decoding failures are Go errors.
func SaveToLinkding(ctx context.Context, request LinkdingRequest) (*Result, error) {
	if !request.Config.IsConfigured() {
		return linkdingResult(false, "Linkding is not configured", nil), nil
	}
	response, err := postBookmark(ctx, request)
	if err != nil {
		return nil, &Error{Message: "Failed to connect to Linkding", Cause: err}
	}
	defer func() { _ = response.Body.Close() }()
	if response.StatusCode < 200 || response.StatusCode > 299 {
		return linkdingResult(false, errorMessage(response), nil), nil
	}
	var created struct {
		ID int64 `json:"id"`
	}
	if err := json.NewDecoder(response.Body).Decode(&created); err != nil {
		return nil, &Error{Message: "Failed to parse Linkding response", Cause: err}
	}
	bookmarkURL := bookmarkPageURL(request.Config.APIURL, created.ID)
	return linkdingResult(true, "Saved to Linkding", &bookmarkURL), nil
}

func postBookmark(ctx context.Context, request LinkdingRequest) (*http.Response, error) {
	body, err := json.Marshal(linkdingBookmark{
		URL: request.Bookmark.URL, Title: request.Bookmark.Title,
		Description: request.Bookmark.Description, TagNames: request.Bookmark.Tags,
	})
	if err != nil {
		return nil, err
	}
	apiURL := NormalizeAPIURL(request.Config.APIURL)
	// The caller's client may follow redirects with the token attached.
	client := *request.Client
	client.CheckRedirect = fetch.DropAuthorizationOnOriginChange
	return fetch.SendWithRetry(ctx, fetch.SendParams{
		Client: &client,
		Config: request.Retry,
		Build: func(ctx context.Context) (*http.Request, error) {
			built, err := http.NewRequestWithContext(ctx, http.MethodPost, apiURL, bytes.NewReader(body))
			if err != nil {
				return nil, err
			}
			built.Header.Set("Authorization", "Token "+request.Config.APIToken)
			built.Header.Set("Content-Type", "application/json")
			return built, nil
		},
	})
}

// errorMessage shows the raw body only for statuses with no specific meaning.
func errorMessage(response *http.Response) string {
	switch response.StatusCode {
	case http.StatusUnauthorized:
		return "Invalid API token"
	case http.StatusForbidden:
		return "Access forbidden"
	case http.StatusNotFound:
		return "Linkding API endpoint not found"
	}
	body, err := io.ReadAll(response.Body)
	text := string(body)
	if err != nil {
		text = "Unknown error"
	}
	if response.StatusCode != http.StatusBadRequest {
		return "Linkding error (" + strconv.Itoa(response.StatusCode) + " " + fetch.StatusReason(response.StatusCode) + "): " + text
	}
	// A 400 for a duplicate means already bookmarked, which the user wants.
	if strings.Contains(text, "already exists") || strings.Contains(text, "unique") {
		return "Bookmark already exists in Linkding"
	}
	return "Bad request: " + text
}

// NormalizeAPIURL turns a pasted URL (root, ".../api" or full) into the
// bookmarks endpoint. The trailing slash is required: without it the redirect
// drops the POST body.
func NormalizeAPIURL(base string) string {
	url := strings.TrimRight(base, "/")
	if !strings.Contains(url, "/api") {
		url += "/api/bookmarks"
	} else if !strings.HasSuffix(url, "/bookmarks") && strings.HasSuffix(url, "/api") {
		url += "/bookmarks"
	}
	if !strings.HasSuffix(url, "/") {
		url += "/"
	}
	return url
}

// bookmarkPageURL is the human-facing page: the setting cut at "/api".
func bookmarkPageURL(base string, id int64) string {
	root := strings.TrimRight(base, "/")
	if position := strings.Index(base, "/api"); position >= 0 {
		root = base[:position]
	}
	return root + "/bookmarks/" + strconv.FormatInt(id, 10)
}
