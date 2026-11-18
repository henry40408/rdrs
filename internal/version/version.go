// Package version reports the build version.
package version

import (
	"runtime/debug"
	"strings"
)

// version is set at link time:
//
//	-ldflags "-X github.com/henry40408/rdrs/internal/version.version=$GIT_VERSION"
//
// Docker builds have no .git directory, so they pass it in.
var version string

// Version returns the injected version, else the VCS-stamped module version,
// else "unknown". An injected "dev" counts as unset.
func Version() string {
	if version != "" && version != "dev" {
		return version
	}
	return buildInfoVersion()
}

// IsDirty reports whether the build came from a tree with uncommitted changes.
// Static assets use it to decide between no-cache and immutable caching.
func IsDirty() bool {
	v := Version()
	// git describe gives "-dirty"; Go stamps "+dirty" into the module version.
	return strings.Contains(v, "-dirty") || strings.Contains(v, "+dirty")
}

func buildInfoVersion() string {
	info, ok := debug.ReadBuildInfo()
	if !ok {
		return "unknown"
	}
	v := info.Main.Version
	if v == "" || v == "(devel)" {
		return "unknown"
	}
	return v
}
