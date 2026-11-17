package server

import (
	"bytes"
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"strconv"
)

// AppError is a failure with the HTTP status and client-visible message it
// maps to. The JSON body is always {"error": message}.
type AppError struct {
	Status  int
	Message string
	// RetryAfter is sent as Retry-After (seconds) when positive.
	RetryAfter int
	// Cause is logged, never sent to the client.
	Cause error
}

func (e *AppError) Error() string { return e.Message }
func (e *AppError) Unwrap() error { return e.Cause }

func newError(status int, message string) *AppError {
	return &AppError{Status: status, Message: message}
}

// Errors whose message is fixed; the statuses follow the Rust AppError table.
var (
	ErrInvalidCredentials     = newError(http.StatusUnauthorized, "Invalid credentials")
	ErrUserNotFound           = newError(http.StatusNotFound, "User not found")
	ErrUsernameExists         = newError(http.StatusConflict, "Username already exists")
	ErrRegistrationNotAllowed = newError(http.StatusForbidden, "Registration not allowed")
	ErrInvalidSetupToken      = newError(http.StatusForbidden, "Invalid setup token")
	ErrUserDisabled           = newError(http.StatusForbidden, "User is disabled")
	ErrUnauthorized           = newError(http.StatusUnauthorized, "Unauthorized")
	ErrForbidden              = newError(http.StatusForbidden, "Forbidden")
	// 403, not 401: the session is valid, and passkey.js matches this message.
	ErrReauthenticationRequired = newError(http.StatusForbidden, "Reauthentication required")
	ErrCannotModifySelf         = newError(http.StatusBadRequest, "Cannot modify self")
	ErrAlreadyMasquerading      = newError(http.StatusBadRequest, "Already masquerading")
	ErrNotMasquerading          = newError(http.StatusBadRequest, "Not masquerading")
	ErrCategoryNotFound         = newError(http.StatusNotFound, "Category not found")
	ErrCategoryExists           = newError(http.StatusConflict, "Category already exists")
	ErrFeedNotFound             = newError(http.StatusNotFound, "Feed not found")
	ErrFeedExists               = newError(http.StatusConflict, "Feed already exists")
	ErrEntryNotFound            = newError(http.StatusNotFound, "Entry not found")
	ErrInvalidURL               = newError(http.StatusBadRequest, "Invalid URL")
	ErrNoFeedFound              = newError(http.StatusBadRequest, "No feed found at URL")
	ErrInvalidImageURL          = newError(http.StatusBadRequest, "Invalid image URL")
	ErrImageTooLarge            = newError(http.StatusBadRequest, "Image too large")
	ErrUnsupportedImageType     = newError(http.StatusBadRequest, "Unsupported image type")
	ErrInvalidSignature         = newError(http.StatusBadRequest, "Invalid signature")
	ErrPasskeyNotFound          = newError(http.StatusNotFound, "Passkey not found")
	ErrChallengeNotFound        = newError(http.StatusBadRequest, "Challenge not found or expired")
)

// FetchError is an upstream fetch failure; the message is shown to the user.
func FetchError(message string) *AppError { return newError(http.StatusBadGateway, message) }

// ValidationError is a rejected input; the message is shown to the user.
func ValidationError(message string) *AppError { return newError(http.StatusBadRequest, message) }

// NotFoundError is a missing resource with a specific message.
func NotFoundError(message string) *AppError { return newError(http.StatusNotFound, message) }

// PasskeyAuthenticationFailed is a failed passkey login.
func PasskeyAuthenticationFailed(message string) *AppError {
	return newError(http.StatusUnauthorized, message)
}

// TooManyRequests carries the rest of the rate-limit window. The message stays
// generic so it reveals nothing about the username, password or account.
func TooManyRequests(retryAfterSecs int) *AppError {
	return &AppError{Status: http.StatusTooManyRequests, Message: "Too many requests", RetryAfter: retryAfterSecs}
}

// DatabaseError hides the cause from the client.
func DatabaseError(cause error) *AppError {
	return &AppError{Status: http.StatusInternalServerError, Message: "Database error", Cause: cause}
}

// InternalError hides the cause from the client.
func InternalError(cause error) *AppError {
	return &AppError{Status: http.StatusInternalServerError, Message: "Internal server error", Cause: cause}
}

// writeError sends err as JSON. A non-AppError is an internal failure.
func writeError(w http.ResponseWriter, err error) {
	var appErr *AppError
	if !errors.As(err, &appErr) {
		appErr = InternalError(err)
	}
	if appErr.Status >= http.StatusInternalServerError && appErr.Cause != nil {
		slog.Error("request failed", "event", "http.error", "status", appErr.Status, "error", appErr.Cause)
	}
	if appErr.RetryAfter > 0 {
		w.Header().Set("Retry-After", strconv.Itoa(appErr.RetryAfter))
	}
	writeJSON(w, appErr.Status, map[string]string{"error": appErr.Message})
}

// writeJSON matches serde_json: compact, no HTML escaping, no trailing newline.
func writeJSON(w http.ResponseWriter, status int, body any) {
	var buf bytes.Buffer
	encoder := json.NewEncoder(&buf)
	encoder.SetEscapeHTML(false)
	if err := encoder.Encode(body); err != nil {
		slog.Error("json encoding failed", "event", "http.json_failed", "error", err)
		w.WriteHeader(http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_, _ = w.Write(bytes.TrimSuffix(buf.Bytes(), []byte("\n")))
}
