package sanitize

import "testing"

// knownDiffs lists inputs whose output intentionally differs from Rust, with
// the reason. See sanitize_known_diffs_test.go.
var knownDiffs = map[string]string{}

// wantRustSanitizeVectors makes dropping a case from rustSanitizeVectors fail
// loudly instead of silently shrinking the coverage.
const wantRustSanitizeVectors = 74

func TestRustSanitizeVectors(t *testing.T) {
	if len(rustSanitizeVectors) != wantRustSanitizeVectors {
		t.Fatalf("%d vectors, want %d", len(rustSanitizeVectors), wantRustSanitizeVectors)
	}
	contexts := map[string]Context{
		"plain": {Secret: []byte(rustSanitizeSecret)},
		"base":  {Secret: []byte(rustSanitizeSecret), BaseURL: "https://example.com/blog/post"},
		"full": {
			Secret:       []byte(rustSanitizeSecret),
			BaseURL:      "https://example.com/blog/post",
			Referrer:     "https://ref.example/",
			ProxyBaseURL: "https://rdrs.example",
		},
	}
	for _, row := range rustSanitizeVectors {
		if reason, ok := knownDiffs[row.input]; ok {
			t.Logf("skipping %q: %s", row.input, reason)
			continue
		}
		for name, ctx := range contexts {
			if got := HTML(row.input, ctx); got != row.outputs[name] {
				t.Errorf("HTML[%s](%q)\n got %q\nwant %q", name, row.input, got, row.outputs[name])
			}
		}
		if got := Summary(row.input); got != row.summary {
			t.Errorf("Summary(%q)\n got %q\nwant %q", row.input, got, row.summary)
		}
	}
}

func TestRustTrackingParamVectors(t *testing.T) {
	for _, r := range rustTrackingParamVectors {
		if got := StripTrackingParams(r.url); got != r.stripped {
			t.Errorf("StripTrackingParams(%q) = %q, want %q", r.url, got, r.stripped)
		}
	}
}
