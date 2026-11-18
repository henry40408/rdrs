package server

import (
	"fmt"
	"strings"
	"testing"
)

func TestParseURLLines(t *testing.T) {
	urls, err := parseURLLines("https://a.com/x\n\n  https://b.com/y  \nhttps://a.com/x\n")
	if err != nil || len(urls) != 2 || urls[0] != "https://a.com/x" || urls[1] != "https://b.com/y" {
		t.Fatalf("urls %v err %v", urls, err)
	}
}

func TestParseURLLinesRefusals(t *testing.T) {
	var many []string
	for i := 0; i < 31; i++ {
		many = append(many, fmt.Sprintf("https://e.com/%d", i))
	}
	cases := map[string]struct{ input, message string }{
		"empty":     {"  \n", "Enter at least one URL."},
		"too many":  {strings.Join(many, "\n"), "30 max"},
		"not a URL": {"nonsense", "Not a valid URL: nonsense"},
		"scheme":    {"ftp://a.com/x", "Only http(s) URLs are supported"},
		"private":   {"http://127.0.0.1/x", "URL not allowed"},
	}
	for name, tc := range cases {
		t.Run(name, func(t *testing.T) {
			_, err := parseURLLines(tc.input)
			if err == nil || !strings.Contains(err.Error(), tc.message) {
				t.Fatalf("err %v, want %q", err, tc.message)
			}
		})
	}
}

func TestURLHost(t *testing.T) {
	if got := urlHost("https://example.com/a?b=1"); got != "example.com" {
		t.Fatal(got)
	}
	if got := urlHost("not a url"); got != "not a url" {
		t.Fatal(got)
	}
}

func TestInflightUsersAllowsOneSlotPerUser(t *testing.T) {
	var slots inflightUsers
	release, ok := slots.begin(1)
	if !ok {
		t.Fatal("first begin refused")
	}
	if _, ok := slots.begin(1); ok {
		t.Fatal("second begin for the same user accepted")
	}
	if other, ok := slots.begin(2); !ok {
		t.Fatal("another user refused")
	} else {
		other()
	}
	release()
	if _, ok := slots.begin(1); !ok {
		t.Fatal("slot not freed")
	}
}
