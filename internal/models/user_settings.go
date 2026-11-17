package models

import (
	"context"
	"errors"
	"strconv"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/secret"
)

var errSettingsMissing = errors.New("failed to retrieve user settings after write")

// Limits and defaults of the per-user settings.
const (
	DefaultEntriesPerPage = 30
	MinEntriesPerPage     = 10
	MaxEntriesPerPage     = 100

	// MaxRetentionReadDays is about ten years; 0 means never delete.
	MaxRetentionReadDays = 3650

	// OfflineKeepOff is offline reading off, the default.
	OfflineKeepOff = 0
	// MaxOfflineKeep caps the entries mirrored offline: it spends the
	// reader's device disk, images included.
	MaxOfflineKeep = 200

	SidebarSortName   = "name"
	SidebarSortUnread = "unread"

	DefaultTimezone = "UTC"
)

// Suggestions rendered as <datalist> options; every one must pass the matching
// update function, and they render in this order.
var (
	EntriesPerPageSuggestions    = []int64{10, 25, 50, 100}
	RetentionReadDaysSuggestions = []int64{0, 7, 30, 90, 365}
	OfflineKeepSuggestions       = []int64{0, 25, 50, 100, 200}
)

// SidebarPrefs are the sidebar display preferences sent together in the
// /api/sidebar payload.
type SidebarPrefs struct {
	Sort     string
	HideRead bool
}

// DefaultSidebarPrefs is what a user without a settings row sees.
func DefaultSidebarPrefs() SidebarPrefs {
	return SidebarPrefs{Sort: SidebarSortName}
}

// ParseSidebarSort maps a sort value to a known ordering. Unknown values fall
// back to the default so a display preference can never break a page render.
func ParseSidebarSort(value string) string {
	if value == SidebarSortUnread {
		return SidebarSortUnread
	}
	return SidebarSortName
}

// UserSettings is a row of the user_settings table.
type UserSettings struct {
	ID                 int64
	UserID             int64
	EntriesPerPage     int64
	RetentionReadDays  int64
	SaveServices       *string
	Theme              *string // "dark", "light", or nil for the system theme
	SidebarSort        string
	SidebarHideRead    bool
	OfflineKeep        int64
	Timezone           string // IANA name
	PixelTrackingSince *time.Time
	CreatedAt          time.Time
	UpdatedAt          time.Time
}

const userSettingsColumns = "id, user_id, entries_per_page, retention_read_days, save_services, theme, " +
	"sidebar_sort, sidebar_hide_read, offline_keep, timezone, pixel_tracking_enabled_at, created_at, updated_at"

func scanUserSettings(row scanner) (UserSettings, error) {
	var s UserSettings
	err := row.Scan(&s.ID, &s.UserID, &s.EntriesPerPage, &s.RetentionReadDays, &s.SaveServices, &s.Theme,
		&s.SidebarSort, &s.SidebarHideRead, &s.OfflineKeep, &s.Timezone, db.NullTimeDest(&s.PixelTrackingSince),
		db.TimeDest(&s.CreatedAt), db.TimeDest(&s.UpdatedAt))
	return s, err
}

// DefaultUserSettings is what a user without a settings row sees.
func DefaultUserSettings(userID int64) UserSettings {
	now := time.Now()
	return UserSettings{
		UserID:         userID,
		EntriesPerPage: DefaultEntriesPerPage,
		SidebarSort:    SidebarSortName,
		OfflineKeep:    OfflineKeepOff,
		Timezone:       DefaultTimezone,
		CreatedAt:      now,
		UpdatedAt:      now,
	}
}

// SidebarPrefs reads the sidebar preferences from an already-loaded row.
func (s *UserSettings) SidebarPrefs() SidebarPrefs {
	return SidebarPrefs{Sort: ParseSidebarSort(s.SidebarSort), HideRead: s.SidebarHideRead}
}

// StoredServices is the decrypted save_services column. Undecryptable is
// distinct from empty so a rotated RDRS_SECRET does not invite the user to
// overwrite a credential that is unreadable, not lost.
type StoredServices struct {
	// JSON is the decrypted document, or "" when nothing is stored. Parsing it
	// belongs to the save services.
	JSON string
	// Undecryptable means a sealed value this key cannot open.
	Undecryptable bool
}

// ErrServicesUndecryptable is the user-facing error for a sealed value that
// the current secret cannot open.
func ErrServicesUndecryptable() error {
	return validationf("Stored service credentials cannot be decrypted with the current RDRS_SECRET. " +
		"Restore the previous secret, or re-enter the credentials in settings.")
}

// Usable returns the JSON, or an error saying the value is undecryptable (not
// "not configured"), for user-triggered paths.
func (s StoredServices) Usable() (string, error) {
	if s.Undecryptable {
		return "", ErrServicesUndecryptable()
	}
	return s.JSON, nil
}

// SaveServicesJSON reads save_services, accepting legacy plaintext JSON until the
// next write seals it. key is nil when RDRS_SECRET was generated.
func (s *UserSettings) SaveServicesJSON(key []byte) StoredServices {
	if s.SaveServices == nil {
		return StoredServices{}
	}
	if !secret.IsSealed(*s.SaveServices) {
		return StoredServices{JSON: *s.SaveServices}
	}
	if key == nil {
		return StoredServices{Undecryptable: true}
	}
	plain, ok := secret.Open(key, *s.SaveServices)
	if !ok {
		return StoredServices{Undecryptable: true}
	}
	return StoredServices{JSON: plain}
}

// FindUserSettings looks a user's settings row up.
func FindUserSettings(ctx context.Context, e db.Executor, userID int64) (*UserSettings, error) {
	settings, err := scanUserSettings(e.QueryRowContext(ctx,
		`SELECT `+userSettingsColumns+` FROM user_settings WHERE user_id = $1`, userID))
	return orNil(&settings, err)
}

// LoadUserSettings is the user's settings, or the defaults when no row exists
// yet. Use it when reading more than one field, so the row is queried once.
func LoadUserSettings(ctx context.Context, e db.Executor, userID int64) (UserSettings, error) {
	settings, err := FindUserSettings(ctx, e, userID)
	if err != nil || settings == nil {
		return DefaultUserSettings(userID), err
	}
	return *settings, nil
}

// reloadUserSettings reads back the row just written; absent means the write
// silently failed.
func reloadUserSettings(ctx context.Context, e db.Executor, userID int64) (*UserSettings, error) {
	settings, err := FindUserSettings(ctx, e, userID)
	if err != nil {
		return nil, err
	}
	if settings == nil {
		return nil, errSettingsMissing
	}
	return settings, nil
}

// EntriesPerPage is the user's page size.
func EntriesPerPage(ctx context.Context, e db.Executor, userID int64) (int64, error) {
	settings, err := LoadUserSettings(ctx, e, userID)
	return settings.EntriesPerPage, err
}

// UpsertUserSettings sets the page size, creating the row if needed.
func UpsertUserSettings(ctx context.Context, e db.Executor, userID, entriesPerPage int64) (*UserSettings, error) {
	if entriesPerPage < MinEntriesPerPage || entriesPerPage > MaxEntriesPerPage {
		return nil, validationf("entries_per_page must be between %d and %d", MinEntriesPerPage, MaxEntriesPerPage)
	}
	_, err := e.ExecContext(ctx,
		`INSERT INTO user_settings (user_id, entries_per_page) VALUES ($1, $2)
		 ON CONFLICT(user_id) DO UPDATE SET entries_per_page = $2, updated_at = $3`,
		userID, entriesPerPage, stamp(time.Now()))
	if err != nil {
		return nil, err
	}
	return reloadUserSettings(ctx, e, userID)
}

// ensureSettingsRow inserts a default row if missing, so a following UPDATE
// has a target.
func ensureSettingsRow(ctx context.Context, e db.Executor, userID int64) error {
	_, err := e.ExecContext(ctx,
		`INSERT INTO user_settings (user_id, entries_per_page) VALUES ($1, $2)
		 ON CONFLICT(user_id) DO NOTHING`, userID, DefaultEntriesPerPage)
	return err
}

// updateSettings sets columns (written as "a = $2, b = $3", with args in that
// order) and updated_at, creating the row first.
func updateSettings(ctx context.Context, e db.Executor, userID int64, set string, args ...any) error {
	if err := ensureSettingsRow(ctx, e, userID); err != nil {
		return err
	}
	stampPlaceholder := "$" + strconv.Itoa(len(args)+2)
	_, err := e.ExecContext(ctx,
		`UPDATE user_settings SET `+set+`, updated_at = `+stampPlaceholder+` WHERE user_id = $1`,
		append([]any{userID}, append(args, stamp(time.Now()))...)...)
	return err
}

// SaveServicesOf reads a user's save_services; see StoredServices.
func SaveServicesOf(ctx context.Context, e db.Executor, userID int64, key []byte) (StoredServices, error) {
	settings, err := FindUserSettings(ctx, e, userID)
	if err != nil || settings == nil {
		return StoredServices{}, err
	}
	return settings.SaveServicesJSON(key), nil
}

// UpdateSaveServices writes a user's save_services JSON, sealed with key if
// given. With no key (a generated RDRS_SECRET, new each boot) it stays
// plaintext, since sealing would lose it on restart.
func UpdateSaveServices(ctx context.Context, e db.Executor, userID int64, json string, key []byte) (*UserSettings, error) {
	stored := json
	if key != nil {
		sealed, err := secret.Seal(key, json)
		if err != nil {
			return nil, err
		}
		stored = sealed
	}
	if err := updateSettings(ctx, e, userID, "save_services = $2", stored); err != nil {
		return nil, err
	}
	return reloadUserSettings(ctx, e, userID)
}

// UpdateTheme sets the theme: "dark", "light", or nil for the system theme.
func UpdateTheme(ctx context.Context, e db.Executor, userID int64, theme *string) error {
	return updateSettings(ctx, e, userID, "theme = $2", theme)
}

// UpdateRetentionReadDays sets read-entry retention in days.
func UpdateRetentionReadDays(ctx context.Context, e db.Executor, userID, days int64) error {
	if days < 0 || days > MaxRetentionReadDays {
		return validationf("retention_read_days must be between 0 and %d", MaxRetentionReadDays)
	}
	return updateSettings(ctx, e, userID, "retention_read_days = $2", days)
}

// UpdateSidebarPrefs sets the sidebar preferences. sort is normalised, not
// rejected.
func UpdateSidebarPrefs(ctx context.Context, e db.Executor, userID int64, prefs SidebarPrefs) error {
	return updateSettings(ctx, e, userID, "sidebar_sort = $2, sidebar_hide_read = $3",
		ParseSidebarSort(prefs.Sort), prefs.HideRead)
}

// OfflineKeep is the offline entry budget; no row means OfflineKeepOff.
func OfflineKeep(ctx context.Context, e db.Executor, userID int64) (int64, error) {
	settings, err := LoadUserSettings(ctx, e, userID)
	return settings.OfflineKeep, err
}

// UpdateOfflineKeep sets the offline budget. Out of range is rejected, not
// clamped, since it spends the reader's disk.
func UpdateOfflineKeep(ctx context.Context, e db.Executor, userID, keep int64) error {
	if keep < OfflineKeepOff || keep > MaxOfflineKeep {
		return validationf("offline_keep must be between %d and %d", OfflineKeepOff, MaxOfflineKeep)
	}
	return updateSettings(ctx, e, userID, "offline_keep = $2", keep)
}

// PixelTrackingEnabledAt is when open tracking was enabled; nil, including no
// row, means opted out.
func PixelTrackingEnabledAt(ctx context.Context, e db.Executor, userID int64) (*time.Time, error) {
	settings, err := FindUserSettings(ctx, e, userID)
	if err != nil || settings == nil {
		return nil, err
	}
	return settings.PixelTrackingSince, nil
}

// UpdatePixelTracking turns open tracking on or off.
//
// Enabling uses COALESCE so re-saving the form does not reset the open-rate
// baseline; disabling clears it but keeps entry_open rows. Both use
// datetime('now'), not a bound time: SQLite would store a bound timestamp in a
// format that does not compare with entry.created_at.
func UpdatePixelTracking(ctx context.Context, e db.Executor, userID int64, enabled bool) error {
	if err := ensureSettingsRow(ctx, e, userID); err != nil {
		return err
	}
	set := "pixel_tracking_enabled_at = NULL"
	if enabled {
		set = "pixel_tracking_enabled_at = COALESCE(pixel_tracking_enabled_at, datetime('now'))"
	}
	_, err := e.ExecContext(ctx, e.Dialect().Rewrite(
		`UPDATE user_settings SET `+set+`, updated_at = datetime('now') WHERE user_id = $1`), userID)
	return err
}
