package models

import (
	"context"
	"errors"
	"slices"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/secret"
)

var testKey = []byte("0123456789abcdef0123456789abcdef")

const servicesJSON = `{"linkding":{"api_url":"https://l.example.com","api_token":"SUPERSECRET123"}}`

func settingsUser(t *testing.T, database *db.DB) int64 {
	t.Helper()
	return mustUser(t, database, "settingsuser").ID
}

func storedServicesColumn(t *testing.T, database *db.DB, userID int64) string {
	t.Helper()
	var column string
	err := database.QueryRowContext(context.Background(),
		`SELECT save_services FROM user_settings WHERE user_id = $1`, userID).Scan(&column)
	if err != nil {
		t.Fatal(err)
	}
	return column
}

func mustLoadSettings(t *testing.T, database *db.DB, userID int64) UserSettings {
	t.Helper()
	settings, err := LoadUserSettings(context.Background(), database, userID)
	if err != nil {
		t.Fatal(err)
	}
	return settings
}

func mustServices(t *testing.T, database *db.DB, userID int64, key []byte) StoredServices {
	t.Helper()
	stored, err := SaveServicesOf(context.Background(), database, userID, key)
	if err != nil {
		t.Fatal(err)
	}
	return stored
}

func mustUpdateServices(t *testing.T, database *db.DB, userID int64, key []byte) {
	t.Helper()
	if _, err := UpdateSaveServices(context.Background(), database, userID, servicesJSON, key); err != nil {
		t.Fatal(err)
	}
}

func isValidation(err error) bool {
	var v *ValidationError
	return errors.As(err, &v)
}

func TestStoredTokenIsNotReadableInTheColumn(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		userID := settingsUser(t, database)
		mustUpdateServices(t, database, userID, testKey)

		column := storedServicesColumn(t, database, userID)
		if strings.Contains(column, "SUPERSECRET123") || !secret.IsSealed(column) {
			t.Errorf("column = %q", column)
		}
		if got := mustServices(t, database, userID, testKey); got.JSON != servicesJSON || got.Undecryptable {
			t.Errorf("round trip = %+v", got)
		}
	})
}

func TestLegacyPlaintextIsReadThenSealedOnNextWrite(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		userID := settingsUser(t, database)
		mustUpdateServices(t, database, userID, nil)
		if !strings.Contains(storedServicesColumn(t, database, userID), "SUPERSECRET123") {
			t.Fatal("without a key the value stays plaintext")
		}
		if got := mustServices(t, database, userID, testKey); got.JSON != servicesJSON {
			t.Errorf("legacy read = %+v", got)
		}
		mustUpdateServices(t, database, userID, testKey)
		if strings.Contains(storedServicesColumn(t, database, userID), "SUPERSECRET123") {
			t.Error("the next write must seal")
		}
	})
}

func TestWrongKeyReportsUndecryptableRatherThanEmpty(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		userID := settingsUser(t, database)
		mustUpdateServices(t, database, userID, testKey)

		for name, key := range map[string][]byte{"other key": []byte("a different key, long enough"), "no key": nil} {
			stored := mustServices(t, database, userID, key)
			if !stored.Undecryptable || stored.JSON != "" {
				t.Errorf("%s: %+v", name, stored)
			}
			if _, err := stored.Usable(); !isValidation(err) {
				t.Errorf("%s: Usable err = %v", name, err)
			}
		}
	})
}

func TestNoStoredServicesReadsAsEmpty(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		userID := settingsUser(t, database)
		if got := mustServices(t, database, userID, testKey); got != (StoredServices{}) {
			t.Errorf("got %+v", got)
		}
	})
}

func TestPixelTrackingLifecycle(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		userID := settingsUser(t, database)
		enabledAt := func() string {
			at, err := PixelTrackingEnabledAt(ctx, database, userID)
			if err != nil {
				t.Fatal(err)
			}
			if at == nil {
				return ""
			}
			return at.String()
		}
		if enabledAt() != "" {
			t.Fatal("a new account must not track")
		}
		if err := UpdatePixelTracking(ctx, database, userID, true); err != nil {
			t.Fatal(err)
		}
		first := enabledAt()
		if first == "" {
			t.Fatal("enabling records a baseline")
		}
		if err := UpdatePixelTracking(ctx, database, userID, true); err != nil {
			t.Fatal(err)
		}
		if enabledAt() != first {
			t.Error("re-enabling moved the baseline")
		}
		if err := UpdatePixelTracking(ctx, database, userID, false); err != nil {
			t.Fatal(err)
		}
		if enabledAt() != "" {
			t.Error("disabling must clear the baseline")
		}
	})
}

func TestOfflineKeepDefaultsRoundTripsAndRejectsOutOfRange(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		userID := settingsUser(t, database)
		if got, _ := OfflineKeep(ctx, database, userID); got != OfflineKeepOff {
			t.Errorf("default = %d", got)
		}
		if err := UpdateOfflineKeep(ctx, database, userID, 50); err != nil {
			t.Fatal(err)
		}
		for _, bad := range []int64{-1, MaxOfflineKeep + 1} {
			if err := UpdateOfflineKeep(ctx, database, userID, bad); !isValidation(err) {
				t.Errorf("%d: err = %v", bad, err)
			}
		}
		if got, _ := OfflineKeep(ctx, database, userID); got != 50 {
			t.Errorf("a rejected write disturbed the value: %d", got)
		}
	})
}

func TestUpsertSettingsValidatesAndPreservesOthers(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		userID := settingsUser(t, database)
		if got, _ := EntriesPerPage(ctx, database, userID); got != DefaultEntriesPerPage {
			t.Errorf("default = %d", got)
		}
		for _, bad := range []int64{5, 150} {
			if _, err := UpsertUserSettings(ctx, database, userID, bad); !isValidation(err) {
				t.Errorf("%d: err = %v", bad, err)
			}
		}
		for _, ok := range []int64{MinEntriesPerPage, MaxEntriesPerPage, 75} {
			settings, err := UpsertUserSettings(ctx, database, userID, ok)
			if err != nil || settings.EntriesPerPage != ok || settings.UserID != userID {
				t.Fatalf("%d: %+v, %v", ok, settings, err)
			}
		}
		found, err := FindUserSettings(ctx, database, userID)
		if err != nil || found == nil || found.EntriesPerPage != 75 || found.Timezone != DefaultTimezone {
			t.Errorf("found = %+v, %v", found, err)
		}
		if missing, err := FindUserSettings(ctx, database, userID+99); err != nil || missing != nil {
			t.Errorf("missing = %+v, %v", missing, err)
		}
	})
}

func TestThemeRoundTripKeepsOtherSettings(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		userID := settingsUser(t, database)
		if mustLoadSettings(t, database, userID).Theme != nil {
			t.Fatal("theme must default to the system theme")
		}
		if _, err := UpsertUserSettings(ctx, database, userID, 50); err != nil {
			t.Fatal(err)
		}
		for _, theme := range []*string{ptr("dark"), ptr("light"), nil} {
			if err := UpdateTheme(ctx, database, userID, theme); err != nil {
				t.Fatal(err)
			}
			got := mustLoadSettings(t, database, userID)
			if (got.Theme == nil) != (theme == nil) || (theme != nil && *got.Theme != *theme) || got.EntriesPerPage != 50 {
				t.Errorf("theme = %v, want %v (%+v)", got.Theme, theme, got)
			}
		}
	})
}

func TestSuggestionsAreAllAcceptedAndSorted(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		userID := settingsUser(t, database)
		for _, v := range EntriesPerPageSuggestions {
			if _, err := UpsertUserSettings(ctx, database, userID, v); err != nil {
				t.Errorf("entries per page %d: %v", v, err)
			}
		}
		for _, v := range RetentionReadDaysSuggestions {
			if err := UpdateRetentionReadDays(ctx, database, userID, v); err != nil {
				t.Errorf("retention %d: %v", v, err)
			} else if got := mustLoadSettings(t, database, userID).RetentionReadDays; got != v {
				t.Errorf("retention = %d, want %d", got, v)
			}
		}
		for _, v := range OfflineKeepSuggestions {
			if err := UpdateOfflineKeep(ctx, database, userID, v); err != nil {
				t.Errorf("offline keep %d: %v", v, err)
			}
		}
	})
	for name, list := range map[string][]int64{
		"entries": EntriesPerPageSuggestions, "retention": RetentionReadDaysSuggestions, "offline": OfflineKeepSuggestions,
	} {
		if !slices.IsSorted(list) {
			t.Errorf("%s suggestions = %v", name, list)
		}
	}
}

func TestRetentionReadDaysBounds(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		userID := settingsUser(t, database)
		if got := mustLoadSettings(t, database, userID).RetentionReadDays; got != 0 {
			t.Errorf("default = %d", got)
		}
		if _, err := UpsertUserSettings(ctx, database, userID, 50); err != nil {
			t.Fatal(err)
		}
		if err := UpdateRetentionReadDays(ctx, database, userID, 14); err != nil {
			t.Fatal(err)
		}
		if got := mustLoadSettings(t, database, userID); got.RetentionReadDays != 14 || got.EntriesPerPage != 50 {
			t.Errorf("got %+v", got)
		}
		for _, bad := range []int64{-1, MaxRetentionReadDays + 1} {
			if err := UpdateRetentionReadDays(ctx, database, userID, bad); !isValidation(err) {
				t.Errorf("%d: err = %v", bad, err)
			}
		}
		if err := UpdateRetentionReadDays(ctx, database, userID, MaxRetentionReadDays); err != nil {
			t.Error(err)
		}
	})
}

func TestSidebarPrefs(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		userID := settingsUser(t, database)
		settings := mustLoadSettings(t, database, userID)
		if settings.SidebarPrefs() != DefaultSidebarPrefs() {
			t.Errorf("default = %+v", settings.SidebarPrefs())
		}
		if _, err := UpsertUserSettings(ctx, database, userID, 50); err != nil {
			t.Fatal(err)
		}
		want := SidebarPrefs{Sort: SidebarSortUnread, HideRead: true}
		if err := UpdateSidebarPrefs(ctx, database, userID, want); err != nil {
			t.Fatal(err)
		}
		settings = mustLoadSettings(t, database, userID)
		if settings.SidebarPrefs() != want || settings.EntriesPerPage != 50 {
			t.Errorf("got %+v", settings)
		}
		// An unknown value is stored as the default, never verbatim.
		if err := UpdateSidebarPrefs(ctx, database, userID, SidebarPrefs{Sort: "nonsense"}); err != nil {
			t.Fatal(err)
		}
		if got := mustLoadSettings(t, database, userID); got.SidebarSort != SidebarSortName || got.SidebarHideRead {
			t.Errorf("got %+v", got)
		}
	})
}
