package greader

import (
	"errors"
	"math"
	"testing"
	"time"
)

func TestParseStream(t *testing.T) {
	cases := map[string]Stream{
		"user/-/state/com.google/reading-list": {Kind: ReadingList},
		"user/-/state/com.google/read":         {Kind: Read},
		"user/-/state/com.google/starred":      {Kind: Starred},
		"user/-/state/com.google/kept-unread":  {Kind: KeptUnread},
		"user/-/label/Tech":                    {Kind: Label, Name: "Tech"},
		"user/12345/label/Tech":                {Kind: Label, Name: "Tech"},
		"user/12345/state/com.google/read":     {Kind: Read},
		"feed/https://example.com/rss":         {Kind: Feed, Name: "https://example.com/rss"},
	}
	for raw, want := range cases {
		got, err := ParseStream(raw)
		if err != nil || got != want {
			t.Errorf("ParseStream(%q) = %+v, %v; want %+v", raw, got, err, want)
		}
	}
}

func TestParseStreamRefusals(t *testing.T) {
	cases := map[string]string{
		"feed/":                       "Empty feed URL in stream ID",
		"user/-/label/":               "Empty label name",
		"user/-/state/com.google/foo": "Unknown stream ID: user/-/state/com.google/foo",
		"user/-":                      "Invalid stream ID: user/-",
		"nonsense":                    "Invalid stream ID: nonsense",
		"user/-/other":                "Unknown stream ID: user/-/other",
	}
	for raw, message := range cases {
		_, err := ParseStream(raw)
		if err == nil || err.Error() != message {
			t.Errorf("ParseStream(%q) error = %v, want %q", raw, err, message)
		}
	}
}

func TestStreamStringRoundTrips(t *testing.T) {
	for _, raw := range []string{
		ReadingListID, ReadID, StarredID, KeptUnreadID, "user/-/label/Tech", "feed/https://example.com/rss",
	} {
		stream, err := ParseStream(raw)
		if err != nil || stream.String() != raw {
			t.Errorf("%q -> %+v -> %q (%v)", raw, stream, stream.String(), err)
		}
	}
}

func TestLabelName(t *testing.T) {
	cases := map[string]struct {
		name string
		ok   bool
	}{
		"user/-/label/Foo":   {"Foo", true},
		"user/123/label/Bar": {"Bar", true},
		"tag/something":      {"", false},
		"user/-/state/read":  {"", false},
	}
	for raw, want := range cases {
		name, ok := LabelName(raw)
		if name != want.name || ok != want.ok {
			t.Errorf("LabelName(%q) = %q, %v", raw, name, ok)
		}
	}
}

func TestItemIDForms(t *testing.T) {
	if got := ItemID(255); got != "tag:google.com,2005:reader/item/00000000000000ff" {
		t.Fatalf("ItemID = %q", got)
	}
	for raw, want := range map[string]int64{
		"tag:google.com,2005:reader/item/00000000000000ff": 255,
		"tag:google.com,2005:reader/item/ff":               255,
		"12345":                                            12345,
		"00000000000000ff":                                 255,
		"ff":                                               255,
		"tag:google.com,2005:reader/item/":                 0,
	} {
		got, err := EntryID(raw)
		if err != nil || got != want {
			t.Errorf("EntryID(%q) = %d, %v; want %d", raw, got, err, want)
		}
	}
	if _, err := EntryID("zz"); err == nil {
		t.Error("EntryID(zz) succeeded")
	}
}

func TestItemIDRoundTrips(t *testing.T) {
	for _, id := range []int64{0, 1, 42, 1000, math.MaxInt64} {
		got, err := EntryID(ItemID(id))
		if err != nil || got != id {
			t.Errorf("%d -> %q -> %d (%v)", id, ItemID(id), got, err)
		}
	}
}

var (
	rootSecret = []byte("0123456789abcdef0123456789abcdef")
	issuedAt   = time.Unix(1_700_000_000, 0)
)

func TestPostTokenRoundTrip(t *testing.T) {
	token := GeneratePostToken(rootSecret, "session-1", issuedAt)
	if err := VerifyPostToken(rootSecret, "session-1", token, issuedAt.Add(time.Minute)); err != nil {
		t.Fatal(err)
	}
}

func TestPostTokenRefusals(t *testing.T) {
	token := GeneratePostToken(rootSecret, "session-1", issuedAt)
	cases := map[string]struct {
		secret  []byte
		subject string
		token   string
		now     time.Time
	}{
		"wrong secret":  {[]byte("another-secret-another-secret-123"), "session-1", token, issuedAt},
		"wrong subject": {rootSecret, "session-2", token, issuedAt},
		"no slash":      {rootSecret, "session-1", "garbage", issuedAt},
		"bad timestamp": {rootSecret, "session-1", "abc/deadbeef", issuedAt},
		"bad hex":       {rootSecret, "session-1", "1700000000/zz", issuedAt},
		"expired":       {rootSecret, "session-1", token, issuedAt.Add(PostTokenValidity + time.Second)},
	}
	for name, c := range cases {
		if err := VerifyPostToken(c.secret, c.subject, c.token, c.now); !errors.Is(err, ErrBadPostToken) {
			t.Errorf("%s: %v", name, err)
		}
	}
	// The edge of the window is still valid.
	if err := VerifyPostToken(rootSecret, "session-1", token, issuedAt.Add(PostTokenValidity)); err != nil {
		t.Errorf("at the limit: %v", err)
	}
}
