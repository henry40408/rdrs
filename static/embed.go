// Package static embeds the front-end assets served under /static/.
package static

import "embed"

// FS holds every asset; the server serves only the names it allow-lists.
//
//go:embed css fonts icons js manifest.webmanifest
var FS embed.FS
