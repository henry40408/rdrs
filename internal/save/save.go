// Package save sends an entry to the user's external bookmark services.
package save

import (
	"encoding/json"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/summarize"
)

// BookmarkData is what is sent to a service.
type BookmarkData struct {
	URL         string
	Title       *string
	Description *string
	Tags        []string
}

// Result is the outcome of one service's save, shown per service in the UI.
type Result struct {
	Success     bool    `json:"success"`
	Service     string  `json:"service"`
	Message     string  `json:"message"`
	BookmarkURL *string `json:"bookmark_url"`
}

// Error is a transport or decoding failure, worded as the Rust server did.
type Error struct {
	Message string
	Cause   error
}

func (e *Error) Error() string { return e.Message + ": " + e.Cause.Error() }

func (e *Error) Unwrap() error { return e.Cause }

// ServicesConfig is every service's setting, stored as JSON in
// user_settings.save_services. Kagi lives here too, though it summarizes
// rather than saves.
type ServicesConfig struct {
	Linkding *LinkdingConfig       `json:"linkding"`
	Kagi     *summarize.KagiConfig `json:"kagi"`
}

// ParseServicesConfig reads the stored JSON.
func ParseServicesConfig(stored string) (ServicesConfig, error) {
	var config ServicesConfig
	err := json.Unmarshal([]byte(stored), &config)
	return config, err
}

// ConfigOf reads the document of a StoredServices. An empty or unparsable
// document is an empty config, so a corrupt value never blocks the settings
// page; an undecryptable one is also empty, for read-only paths. Writers must
// check StoredServices.Undecryptable first.
func ConfigOf(stored models.StoredServices) ServicesConfig {
	config, err := ParseServicesConfig(stored.JSON)
	if err != nil {
		return ServicesConfig{}
	}
	return config
}

// UsableConfigOf is ConfigOf for user-triggered paths: an undecryptable value
// is an error, not "not configured".
func UsableConfigOf(stored models.StoredServices) (ServicesConfig, error) {
	if _, err := stored.Usable(); err != nil {
		return ServicesConfig{}, err
	}
	return ConfigOf(stored), nil
}

// JSON is the form written back to the database.
func (c ServicesConfig) JSON() (string, error) {
	encoded, err := json.Marshal(c)
	return string(encoded), err
}

// ConfiguredServices lists the saving services whose credentials are filled in.
func (c ServicesConfig) ConfiguredServices() []string {
	services := []string{}
	if c.Linkding != nil && c.Linkding.IsConfigured() {
		services = append(services, "linkding")
	}
	return services
}

// HasAnyService reports whether a save button should show at all.
func (c ServicesConfig) HasAnyService() bool {
	return len(c.ConfiguredServices()) > 0
}
