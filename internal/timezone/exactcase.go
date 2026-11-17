package timezone

import (
	"os"
	"strings"
)

// hostZoneinfoDirs are the directories time.LoadLocation reads before it
// falls back to the embedded database.
var hostZoneinfoDirs = []string{
	"/usr/share/zoneinfo",
	"/usr/share/lib/zoneinfo",
	"/usr/lib/locale/TZ",
	"/etc/zoneinfo",
}

// hasExactCase reports whether name is spelled exactly as the host's zoneinfo
// files are. LoadLocation reads the host first, and a case-insensitive file
// system (macOS) would accept "ASIA/TAIPEI" and store it as typed, which a
// case-sensitive host then fails to load. Without a host directory the
// embedded database decides, and it is exact.
func hasExactCase(name string) bool {
	for _, dir := range hostZoneinfoDirs {
		if _, err := os.Stat(dir); err != nil {
			continue
		}
		return entryExists(dir, strings.Split(name, "/"))
	}
	return true
}

// entryExists checks each segment against the real directory listing, which
// keeps the file system's own spelling.
func entryExists(dir string, segments []string) bool {
	for i, segment := range segments {
		entries, err := os.ReadDir(dir)
		if err != nil {
			return false
		}
		found := false
		for _, entry := range entries {
			if entry.Name() == segment {
				found = true
				break
			}
		}
		if !found {
			return false
		}
		if i < len(segments)-1 {
			dir += "/" + segment
		}
	}
	return true
}
