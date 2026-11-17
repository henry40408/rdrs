// Package migrations embeds the SQL migration files. The files are shared with
// the Rust build, so they must never be reformatted: their SHA-384 checksums
// are recorded in every existing database.
package migrations

import "embed"

// FS holds sqlite/*.sql and postgres/*.sql.
//
//go:embed sqlite/*.sql postgres/*.sql
var FS embed.FS
