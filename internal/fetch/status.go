package fetch

import "net/http"

// statusReasons are the three phrases where Go's names differ from the http
// crate's, which Rust showed.
var statusReasons = map[int]string{
	http.StatusRequestEntityTooLarge:        "Payload Too Large",
	http.StatusRequestURITooLong:            "URI Too Long",
	http.StatusRequestedRangeNotSatisfiable: "Range Not Satisfiable",
}

// StatusReason is the reason phrase the Rust server showed users, including
// its "<unknown status code>" for codes without one.
func StatusReason(code int) string {
	if text, ok := statusReasons[code]; ok {
		return text
	}
	if text := http.StatusText(code); text != "" {
		return text
	}
	return "<unknown status code>"
}
