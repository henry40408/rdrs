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

// hasExactCase reports whether name is spelled the way the host's zoneinfo
// files are. LoadLocation reads the host first, and a case-insensitive file
// system (macOS) would accept "ASIA/TAIPEI" and store it as typed, which a
// case-sensitive host then fails to load. Without a host directory the
// embedded database decides, and it is exact.
func hasExactCase(name string) bool {
	for _, dir := range hostZoneinfoDirs {
		if _, err := os.Stat(dir); err != nil {
			continue
		}
		return !hasWrongCaseSegment(dir, strings.Split(name, "/"))
	}
	return true
}

// hasWrongCaseSegment finds a segment the host spells differently only in
// case. A segment the host does not know at all is not wrong: minimal hosts
// (CI images) leave out legacy aliases such as Asia/Calcutta that the embedded
// database still has, so the embedded database decides.
func hasWrongCaseSegment(dir string, segments []string) bool {
	for _, segment := range segments {
		entries, err := os.ReadDir(dir)
		if err != nil {
			return false
		}
		exact, folded := false, false
		for _, entry := range entries {
			exact = exact || entry.Name() == segment
			folded = folded || strings.EqualFold(entry.Name(), segment)
		}
		if !exact {
			return folded
		}
		dir += "/" + segment
	}
	return false
}
