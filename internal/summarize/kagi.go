// Package summarize produces entry summaries through the Kagi Universal
// Summarizer and runs the background worker that does so.
package summarize

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"

	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/save"
)

const kagiAPIBase = "https://kagi.com"

// KagiConfig is the user's Kagi setting. It lives in save, next to the rest of
// the stored services document.
type KagiConfig = save.KagiConfig

// Result is the outcome of one summarization. A Kagi-side problem comes back as
// Success false so it cannot fail the caller; only transport and decoding
// failures are Go errors.
type Result struct {
	Success    bool
	OutputText *string
	Error      *string
	Title      *string
}

// Error is a transport or decoding failure, worded as the Rust server did.
type Error struct {
	Message string
	Cause   error
}

func (e *Error) Error() string { return e.Message + ": " + e.Cause.Error() }

func (e *Error) Unwrap() error { return e.Cause }

func failure(message string) *Result {
	return &Result{Error: &message}
}

// Request is one Kagi call.
type Request struct {
	Config KagiConfig
	URL    string
	// Base overrides the API host; empty means RDRS_KAGI_API_BASE, then kagi.com.
	// The variable points at a local stub for tests and E2E, never production.
	Base   string
	Client *http.Client
	Retry  fetch.RetryConfig
}

type kagiResponse struct {
	OutputData *struct {
		Markdown *string `json:"markdown"`
	} `json:"output_data"`
	Error *string `json:"error"`
}

// Summarize asks Kagi for a summary of the URL.
func Summarize(ctx context.Context, request Request) (*Result, error) {
	if !request.Config.IsConfigured() {
		return failure("Kagi is not configured"), nil
	}
	response, err := send(ctx, request)
	if err != nil {
		return nil, &Error{Message: "Failed to connect to Kagi", Cause: err}
	}
	defer func() { _ = response.Body.Close() }()
	if response.StatusCode < 200 || response.StatusCode > 299 {
		return failure(statusMessage(response)), nil
	}
	var body kagiResponse
	if err := json.NewDecoder(response.Body).Decode(&body); err != nil {
		return nil, &Error{Message: "Failed to parse Kagi response", Cause: err}
	}
	return resultOf(body), nil
}

func send(ctx context.Context, request Request) (*http.Response, error) {
	apiURL := summaryURL(request)
	return fetch.SendWithRetry(ctx, fetch.SendParams{
		Client: request.Client,
		Config: request.Retry,
		Build: func(ctx context.Context) (*http.Request, error) {
			built, err := http.NewRequestWithContext(ctx, http.MethodGet, apiURL, nil)
			if err != nil {
				return nil, err
			}
			built.Header.Set("Authorization", request.Config.SessionToken)
			built.Header.Set("Content-Type", "application/json")
			return built, nil
		},
	})
}

// summaryURL keeps the parameters in the order Kagi's own client sends them.
func summaryURL(request Request) string {
	base := request.Base
	if base == "" {
		base = os.Getenv("RDRS_KAGI_API_BASE")
	}
	if base == "" {
		base = kagiAPIBase
	}
	query := "summary_type=summary&url=" + url.QueryEscape(request.URL)
	if language := request.Config.Language; language != nil && *language != "" {
		query += "&target_language=" + url.QueryEscape(*language)
	}
	return base + "/mother/summary_labs?" + query
}

func statusMessage(response *http.Response) string {
	switch response.StatusCode {
	case http.StatusUnauthorized:
		return "Invalid session token"
	case http.StatusForbidden:
		return "Access forbidden - check your Kagi subscription"
	case http.StatusTooManyRequests:
		return "Rate limit exceeded - please try again later"
	}
	text, err := io.ReadAll(response.Body)
	if err != nil {
		text = []byte("Unknown error")
	}
	return fmt.Sprintf("Kagi error (%d %s): %s", response.StatusCode, fetch.StatusReason(response.StatusCode), text)
}

func resultOf(body kagiResponse) *Result {
	if body.Error != nil {
		return failure(*body.Error)
	}
	if body.OutputData == nil || body.OutputData.Markdown == nil {
		return failure("No summary returned from Kagi")
	}
	title, text := splitTitle(*body.OutputData.Markdown)
	return &Result{Success: true, OutputText: &text, Title: title}
}

// splitTitle takes off a leading "Title: <t>" line and its blank line. Without
// the blank line the text is left whole.
func splitTitle(markdown string) (*string, string) {
	rest, found := strings.CutPrefix(markdown, "Title: ")
	if !found {
		return nil, markdown
	}
	title, body, found := strings.Cut(rest, "\n\n")
	if !found {
		return nil, markdown
	}
	return &title, body
}
