package sanitize

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func readVectors(t *testing.T, name string, into any) {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join("..", "..", "testdata", "vectors", name))
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(raw, into); err != nil {
		t.Fatal(err)
	}
}

// knownDiffs lists inputs whose output intentionally differs from Rust, with
// the reason. See sanitize_known_diffs_test.go.
var knownDiffs = map[string]string{}

func TestRustSanitizeVectors(t *testing.T) {
	var v struct {
		Secret string `json:"secret"`
		Rows   []struct {
			Input   string            `json:"input"`
			Outputs map[string]string `json:"outputs"`
			Summary string            `json:"summary"`
		} `json:"rows"`
	}
	readVectors(t, "sanitize.json", &v)
	if len(v.Rows) < 70 {
		t.Fatalf("only %d vectors", len(v.Rows))
	}
	contexts := map[string]Context{
		"plain": {Secret: []byte(v.Secret)},
		"base":  {Secret: []byte(v.Secret), BaseURL: "https://example.com/blog/post"},
		"full": {
			Secret:       []byte(v.Secret),
			BaseURL:      "https://example.com/blog/post",
			Referrer:     "https://ref.example/",
			ProxyBaseURL: "https://rdrs.example",
		},
	}
	for _, row := range v.Rows {
		if reason, ok := knownDiffs[row.Input]; ok {
			t.Logf("skipping %q: %s", row.Input, reason)
			continue
		}
		for name, ctx := range contexts {
			if got := HTML(row.Input, ctx); got != row.Outputs[name] {
				t.Errorf("HTML[%s](%q)\n got %q\nwant %q", name, row.Input, got, row.Outputs[name])
			}
		}
		if got := Summary(row.Input); got != row.Summary {
			t.Errorf("Summary(%q)\n got %q\nwant %q", row.Input, got, row.Summary)
		}
	}
}

func TestRustTrackingParamVectors(t *testing.T) {
	var rows []struct {
		URL      string `json:"url"`
		Stripped string `json:"stripped"`
	}
	readVectors(t, "tracking_params.json", &rows)
	for _, r := range rows {
		if got := StripTrackingParams(r.URL); got != r.Stripped {
			t.Errorf("StripTrackingParams(%q) = %q, want %q", r.URL, got, r.Stripped)
		}
	}
}
