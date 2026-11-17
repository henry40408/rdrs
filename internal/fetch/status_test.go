package fetch

import "testing"

func TestStatusReason(t *testing.T) {
	for code, want := range map[int]string{404: "Not Found", 413: "Payload Too Large", 520: "<unknown status code>"} {
		if got := StatusReason(code); got != want {
			t.Errorf("%d: %q, want %q", code, got, want)
		}
	}
}
