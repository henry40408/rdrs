// Package flash carries one-shot banner messages across a redirect in a
// signed cookie.
package flash

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"time"
)

// CookieName is the flash cookie.
const CookieName = "flash"

// maxMessages is how many of the newest messages survive a read.
const maxMessages = 3

// Level is the severity of a message.
type Level string

// The four levels.
const (
	Success Level = "success"
	Error   Level = "error"
	Info    Level = "info"
	Warning Level = "warning"
)

// Kind is what a message carries beyond its text; /admin reads InviteLink
// instead of guessing from the message.
type Kind string

// The two kinds. Plain is the zero value and is left out of the cookie.
const (
	Plain Kind = ""
	// InviteLinkKind marks a one-time invite URL, shown once on /admin.
	InviteLinkKind Kind = "invite_link"
)

// Message is one banner.
type Message struct {
	Level     Level
	Text      string
	Timestamp time.Time
	Kind      Kind
}

// wire is the cookie's JSON form, the one the Rust server wrote.
type wire struct {
	Level     Level  `json:"level"`
	Message   string `json:"message"`
	Timestamp string `json:"timestamp,omitempty"`
	Kind      Kind   `json:"kind,omitempty"`
}

func newMessage(level Level, text string) Message {
	return Message{Level: level, Text: text, Timestamp: time.Now().UTC()}
}

// NewSuccess is a success message.
func NewSuccess(text string) Message { return newMessage(Success, text) }

// NewError is an error message.
func NewError(text string) Message { return newMessage(Error, text) }

// NewInfo is an info message.
func NewInfo(text string) Message { return newMessage(Info, text) }

// NewWarning is a warning message.
func NewWarning(text string) Message { return newMessage(Warning, text) }

// NewInviteLink is a one-time invite URL.
func NewInviteLink(url string) Message {
	message := newMessage(Success, url)
	message.Kind = InviteLinkKind
	return message
}

// LevelClass is the banner CSS class.
func (m Message) LevelClass() string { return "banner--" + string(m.Level) }

// AriaRole is the ARIA live-region role: status (polite) or alert (assertive).
func (m Message) AriaRole() string {
	if m.Level == Warning || m.Level == Error {
		return "alert"
	}
	return "status"
}

// LevelName is the level name screen readers announce.
func (m Message) LevelName() string {
	return strings.ToUpper(string(m.Level[:1])) + string(m.Level[1:])
}

// FormattedTime is UTC HH:MM:SS, the no-JS fallback; rdrs-flash.js localizes
// it from TimestampISO. Do not add a timezone-dependent format here.
func (m Message) FormattedTime() string { return m.Timestamp.UTC().Format("15:04:05") }

// TimestampISO is the ISO-8601 instant for <time datetime>, with the offset
// spelled +00:00 as chrono's to_rfc3339 does.
func (m Message) TimestampISO() string { return formatTimestamp(m.Timestamp, "+00:00") }

// formatTimestamp prints the instant with 0, 3, 6 or 9 fractional digits, the
// shortest that is exact, as chrono does.
func formatTimestamp(t time.Time, zone string) string {
	t = t.UTC()
	layout := "2006-01-02T15:04:05"
	switch nanos := t.Nanosecond(); {
	case nanos == 0:
	case nanos%1_000_000 == 0:
		layout += ".000"
	case nanos%1_000 == 0:
		layout += ".000000"
	default:
		layout += ".000000000"
	}
	return t.Format(layout) + zone
}

func (l *Level) validate() error {
	switch *l {
	case Success, Error, Info, Warning:
		return nil
	}
	return fmt.Errorf("unknown flash level %q", string(*l))
}

func decodeMessages(raw string) ([]Message, error) {
	var items []wire
	if err := json.Unmarshal([]byte(raw), &items); err != nil {
		return nil, err
	}
	messages := make([]Message, 0, len(items))
	for _, item := range items {
		if err := item.Level.validate(); err != nil {
			return nil, err
		}
		if item.Kind != Plain && item.Kind != InviteLinkKind {
			return nil, fmt.Errorf("unknown flash kind %q", string(item.Kind))
		}
		timestamp := time.Now().UTC()
		if item.Timestamp != "" {
			parsed, err := time.Parse(time.RFC3339Nano, item.Timestamp)
			if err != nil {
				return nil, err
			}
			timestamp = parsed.UTC()
		}
		messages = append(messages, Message{Level: item.Level, Text: item.Message, Timestamp: timestamp, Kind: item.Kind})
	}
	return messages, nil
}

func encodeMessages(messages []Message) (string, error) {
	items := make([]wire, len(messages))
	for i, m := range messages {
		items[i] = wire{Level: m.Level, Message: m.Text, Timestamp: formatTimestamp(m.Timestamp, "Z"), Kind: m.Kind}
	}
	var out strings.Builder
	encoder := json.NewEncoder(&out)
	encoder.SetEscapeHTML(false)
	if err := encoder.Encode(items); err != nil {
		return "", err
	}
	return strings.TrimSuffix(out.String(), "\n"), nil
}

// Set queues messages for the next request that reads them.
func Set(w http.ResponseWriter, messages ...Message) {
	raw, err := encodeMessages(messages)
	if err != nil {
		return
	}
	http.SetCookie(w, &http.Cookie{
		Name:     CookieName,
		Value:    escapeValue(raw),
		Path:     "/",
		HttpOnly: true,
		SameSite: http.SameSiteLaxMode,
	})
}

// Take reads the pending messages (at most the newest three) and clears the
// cookie. A cookie the request does not carry is not cleared, so no needless
// Set-Cookie goes out.
func Take(w http.ResponseWriter, r *http.Request) []Message {
	cookie, err := r.Cookie(CookieName)
	if err != nil {
		return nil
	}
	// Path=/ is required, or the removal fails on multi-segment routes.
	http.SetCookie(w, &http.Cookie{Name: CookieName, Path: "/", MaxAge: -1})

	raw, err := url.PathUnescape(cookie.Value)
	if err != nil {
		return nil
	}
	messages, err := decodeMessages(raw)
	if err != nil {
		return nil
	}
	if len(messages) > maxMessages {
		messages = messages[len(messages)-maxMessages:]
	}
	return messages
}

// Redirect sends the client to location with a 303 and queues a message.
func Redirect(w http.ResponseWriter, location string, message Message) {
	Set(w, message)
	w.Header().Set("Location", location)
	w.WriteHeader(http.StatusSeeOther)
}

// cookieEscaped are the bytes the Rust cookie encoder escapes beyond controls
// and non-ASCII.
const cookieEscaped = " \"<>`#?{}/:;=@[\\]^|%(),"

// escapeValue percent-encodes a cookie value the way the Rust server did, so
// the value stays inside what every client accepts.
func escapeValue(value string) string {
	var out strings.Builder
	for i := 0; i < len(value); i++ {
		b := value[i]
		if b < 0x20 || b >= 0x7f || strings.IndexByte(cookieEscaped, b) >= 0 {
			fmt.Fprintf(&out, "%%%02X", b)
			continue
		}
		out.WriteByte(b)
	}
	return out.String()
}
