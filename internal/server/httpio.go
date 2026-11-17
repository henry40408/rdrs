package server

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"mime"
	"net/http"
	"net/url"
	"strings"

	"github.com/henry40408/rdrs/internal/models"
)

// maxBodyBytes caps a decoded request body, as axum's default 2 MiB limit.
const maxBodyBytes = 2 << 20

// rejection is a plain-text refusal of a malformed request, the way axum's
// extractors answer it (not the JSON error body of AppError).
func rejection(w http.ResponseWriter, status int, message string) {
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.WriteHeader(status)
	_, _ = io.WriteString(w, message)
}

func hasMediaType(r *http.Request, want string) bool {
	mediaType, _, err := mime.ParseMediaType(r.Header.Get("Content-Type"))
	return err == nil && mediaType == want
}

// decodeJSON reads a JSON body into out. It reports false after answering.
func decodeJSON(w http.ResponseWriter, r *http.Request, out any) bool {
	if !hasMediaType(r, "application/json") && !hasJSONSuffix(r) {
		rejection(w, http.StatusUnsupportedMediaType, "Expected request with `Content-Type: application/json`")
		return false
	}
	body, ok := readBody(w, r)
	if !ok {
		return false
	}
	if err := json.Unmarshal(body, out); err != nil {
		rejectJSON(w, err)
		return false
	}
	return true
}

// readBody buffers the capped body. Only exceeding the cap is a 413; a client
// that hangs up or sends broken framing is a plain 400.
func readBody(w http.ResponseWriter, r *http.Request) ([]byte, bool) {
	body, err := io.ReadAll(http.MaxBytesReader(w, r.Body, maxBodyBytes))
	if err == nil {
		return body, true
	}
	var tooLarge *http.MaxBytesError
	if errors.As(err, &tooLarge) {
		rejection(w, http.StatusRequestEntityTooLarge, "Failed to buffer the request body")
		return nil, false
	}
	rejection(w, http.StatusBadRequest, "Failed to buffer the request body")
	return nil, false
}

// hasJSONSuffix accepts application/*+json like axum.
func hasJSONSuffix(r *http.Request) bool {
	mediaType, _, err := mime.ParseMediaType(r.Header.Get("Content-Type"))
	return err == nil && strings.HasPrefix(mediaType, "application/") && strings.HasSuffix(mediaType, "+json")
}

func rejectJSON(w http.ResponseWriter, err error) {
	var typeErr *json.UnmarshalTypeError
	if errors.As(err, &typeErr) {
		rejection(w, http.StatusUnprocessableEntity,
			"Failed to deserialize the JSON body into the target type: "+typeErr.Field+": invalid type")
		return
	}
	rejection(w, http.StatusBadRequest, "Failed to parse the request body as JSON: "+err.Error())
}

// parseForm reads an application/x-www-form-urlencoded body. It reports
// false after answering.
func parseForm(w http.ResponseWriter, r *http.Request) (url.Values, bool) {
	if !hasMediaType(r, "application/x-www-form-urlencoded") {
		rejection(w, http.StatusUnsupportedMediaType,
			"Form requests must have `Content-Type: application/x-www-form-urlencoded`")
		return nil, false
	}
	body, ok := readBody(w, r)
	if !ok {
		return nil, false
	}
	values, err := url.ParseQuery(string(body))
	if err != nil {
		rejection(w, http.StatusUnprocessableEntity, "Failed to deserialize form body: "+err.Error())
		return nil, false
	}
	return values, true
}

// optionalForm reads a urlencoded body that may be absent or of another type
// (the forms whose only field is return_to); it never answers.
func optionalForm(r *http.Request) (url.Values, bool) {
	if !hasMediaType(r, "application/x-www-form-urlencoded") {
		return nil, false
	}
	body, err := io.ReadAll(io.LimitReader(r.Body, maxBodyBytes))
	if err != nil {
		return nil, false
	}
	values, err := url.ParseQuery(string(body))
	return values, err == nil
}

// requireFields answers 422 naming the first absent field, like serde does
// for a struct field with no default.
func requireFields(w http.ResponseWriter, form url.Values, names ...string) bool {
	for _, name := range names {
		if !form.Has(name) {
			rejection(w, http.StatusUnprocessableEntity,
				fmt.Sprintf("Failed to deserialize form body: missing field `%s`", name))
			return false
		}
	}
	return true
}

// writeHTML sends a rendered page.
func writeHTML(w http.ResponseWriter, status int, page string) {
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.WriteHeader(status)
	_, _ = io.WriteString(w, page)
}

// redirect is axum's Redirect::to: 303 See Other.
func redirect(w http.ResponseWriter, location string) {
	w.Header().Set("Location", location)
	w.WriteHeader(http.StatusSeeOther)
}

// appError converts a model failure to the AppError its handler answers with.
func appError(err error) error {
	var validation *models.ValidationError
	switch {
	case errors.As(err, &validation):
		return ValidationError(validation.Message)
	case errors.Is(err, models.ErrUsernameExists):
		return ErrUsernameExists
	case errors.Is(err, models.ErrCategoryExists):
		return ErrCategoryExists
	case errors.Is(err, models.ErrUserNotFound):
		return ErrUserNotFound
	}
	return DatabaseError(err)
}
