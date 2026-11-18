package views

import (
	"strings"
	"testing"
)

func TestSummarizerCardShowsOnlyTheControlsOfItsState(t *testing.T) {
	tests := map[string]struct {
		card    SummarizerCard
		present []string
		absent  []string
	}{
		"completed": {
			SummarizerCard{State: "completed", Summary: "<p>done</p>"},
			[]string{"data-sz-copy", "data-sz-body", "<p>done</p>"},
			[]string{"data-sz-retry", "data-sz-error", "data-sz-status", "pending"},
		},
		"error": {
			SummarizerCard{State: "error", Error: "boom"},
			[]string{"data-sz-retry", "data-sz-error", "Summarization failed: boom"},
			[]string{"data-sz-copy", "data-sz-body", "data-sz-status", "pending"},
		},
		"queued": {
			SummarizerCard{State: "queued"},
			[]string{"data-sz-status", "pending", "Queued"},
			[]string{"data-sz-copy", "data-sz-retry", "data-sz-body", "data-sz-error"},
		},
		"processing": {
			SummarizerCard{State: "processing"},
			[]string{"data-sz-status"},
			[]string{"data-sz-copy", "data-sz-retry", "pending"},
		},
	}
	for name, test := range tests {
		got := string(summarizerCardNode(test.card))
		for _, want := range test.present {
			if !strings.Contains(got, want) {
				t.Errorf("%s card lacks %q: %s", name, want, got)
			}
		}
		for _, unwanted := range test.absent {
			if strings.Contains(got, unwanted) {
				t.Errorf("%s card contains %q: %s", name, unwanted, got)
			}
		}
	}
}
