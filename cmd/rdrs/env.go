package main

import (
	"fmt"
	"strings"
)

// legacyEnvVar pairs a retired environment variable with its replacement.
type legacyEnvVar struct{ old, replacement string }

const signupRemoved = "(removed: self-service registration no longer exists; an admin now creates " +
	"accounts from /admin and hands out a one-time link. The first account is still created " +
	"at /setup on a fresh install)"

// legacyEnvVars lists the retired variables. Update it whenever an environment
// variable is added, renamed or removed: a silently ignored one (say, falling
// back to a random secret) is worse than a startup failure.
var legacyEnvVars = []legacyEnvVar{
	{"SERVER_BIND", "RDRS_SERVER_BIND"},
	{"MULTI_USER_ENABLED", "RDRS_MULTI_USER_ENABLED"},
	{"IMAGE_PROXY_SECRET", "RDRS_SECRET"},
	{"USER_AGENT", "RDRS_USER_AGENT"},
	{"WEBAUTHN_RP_ID", "RDRS_WEBAUTHN_RP_ID"},
	{"WEBAUTHN_RP_ORIGIN", "RDRS_WEBAUTHN_RP_ORIGIN"},
	{"WEBAUTHN_RP_NAME", "RDRS_WEBAUTHN_RP_NAME"},
	{"PUBLIC_BASE_URL", "RDRS_PUBLIC_BASE_URL"},
	{"COOKIE_SECURE", "RDRS_COOKIE_SECURE"},
	{"AUTH_PROXY_HEADER", "RDRS_AUTH_PROXY_HEADER"},
	{"TRUSTED_PROXY_NETWORKS", "RDRS_TRUSTED_PROXY_NETWORKS"},
	{"AUTH_PROXY_USER_CREATION", "RDRS_AUTH_PROXY_USER_CREATION"},
	{"DISABLE_LOCAL_AUTH", "RDRS_DISABLE_LOCAL_AUTH"},
	{"AUTH_PROXY_GROUPS_HEADER", "RDRS_AUTH_PROXY_GROUPS_HEADER"},
	{"AUTH_PROXY_ADMIN_GROUP", "RDRS_AUTH_PROXY_ADMIN_GROUP"},
	{"AUTH_PROXY_LOGOUT_URL", "RDRS_AUTH_PROXY_LOGOUT_URL"},
	{"KAGI_API_BASE", "RDRS_KAGI_API_BASE"},
	{"LOG_FORMAT", "RDRS_LOG_FORMAT"},
	{"RDRS_SIGNUP_ENABLED", signupRemoved},
	{"SIGNUP_ENABLED", signupRemoved},
	// A global RUST_LOG would otherwise be ignored without a word.
	{"RUST_LOG", "RDRS_LOG_LEVEL"},
}

// ensureNoLegacyEnvVars fails when a retired variable is set, even empty.
func ensureNoLegacyEnvVars(lookupEnv func(string) (string, bool)) error {
	var found []string
	for _, legacy := range legacyEnvVars {
		if _, ok := lookupEnv(legacy.old); ok {
			found = append(found, fmt.Sprintf("  %s -> %s", legacy.old, legacy.replacement))
		}
	}
	if len(found) == 0 {
		return nil
	}
	return fmt.Errorf("these environment variables no longer exist; "+
		"rename (or unset) them to continue:\n%s", strings.Join(found, "\n"))
}
