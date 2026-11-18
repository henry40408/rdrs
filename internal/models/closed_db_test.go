package models

import (
	"context"
	"path/filepath"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/config"
)

func TestQueriesReturnTheDatabaseError(t *testing.T) {
	ctx := context.Background()
	database := openAndMigrate(t, filepath.Join(t.TempDir(), "closed.sqlite3"), config.SQLite)
	if err := database.Shutdown(ctx); err != nil {
		t.Fatal(err)
	}
	key := make([]byte, 32)
	ids := []int64{1}
	reportsError := map[string]func() error{
		"CreateUser":         func() error { _, err := CreateUser(ctx, database, "a", "h", RoleUser); return err },
		"FindUserByUsername": func() error { _, err := FindUserByUsername(ctx, database, "a"); return err },
		"FindUserByID":       func() error { _, err := FindUserByID(ctx, database, 1); return err },
		"FindUserForLogin":   func() error { _, _, err := FindUserForLogin(ctx, database, "a"); return err },
		"ListUsers":          func() error { _, err := ListUsers(ctx, database); return err },
		"ChangePassword":     func() error { return ChangePassword(ctx, database, 1, "h") },
		"ChangePasswordTx":   func() error { return ChangePasswordTx(ctx, database, 1, "h") },
		"UpdateRole":         func() error { return UpdateRole(ctx, database, 1, RoleAdmin) },
		"DisableUser":        func() error { return DisableUser(ctx, database, 1) },
		"EnableUser":         func() error { return EnableUser(ctx, database, 1) },
		"DeleteUser":         func() error { return DeleteUser(ctx, database, 1) },
		"CountUsers":         func() error { _, err := CountUsers(ctx, database); return err },
		"CountActiveAdmins":  func() error { _, err := CountActiveAdmins(ctx, database); return err },
		"IssueInvite": func() error {
			_, err := IssueInvite(ctx, database, IssueInviteParams{SecretKey: key, UserID: 1, Token: "t"})
			return err
		},
		"FindInviteByToken": func() error { _, err := FindInviteByToken(ctx, database, key, "t"); return err },
		"RedeemInvite": func() error {
			_, err := RedeemInvite(ctx, database, RedeemInviteParams{InviteID: 1, UserID: 1, PasswordHash: "h"})
			return err
		},
		"RevokeInvitesForUser":    func() error { _, err := RevokeInvitesForUser(ctx, database, 1); return err },
		"FindLiveInviteForUser":   func() error { _, err := FindLiveInviteForUser(ctx, database, 1); return err },
		"DeleteStaleInvites":      func() error { _, err := DeleteStaleInvites(ctx, database); return err },
		"FindUserSettings":        func() error { _, err := FindUserSettings(ctx, database, 1); return err },
		"LoadUserSettings":        func() error { _, err := LoadUserSettings(ctx, database, 1); return err },
		"EntriesPerPage":          func() error { _, err := EntriesPerPage(ctx, database, 1); return err },
		"UpsertUserSettings":      func() error { _, err := UpsertUserSettings(ctx, database, 1, 20); return err },
		"SaveServicesOf":          func() error { _, err := SaveServicesOf(ctx, database, 1, key); return err },
		"UpdateSaveServices":      func() error { _, err := UpdateSaveServices(ctx, database, 1, "{}", key); return err },
		"UpdateTheme":             func() error { return UpdateTheme(ctx, database, 1, nil) },
		"UpdateRetentionReadDays": func() error { return UpdateRetentionReadDays(ctx, database, 1, 30) },
		"UpdateSidebarPrefs":      func() error { return UpdateSidebarPrefs(ctx, database, 1, DefaultSidebarPrefs()) },
		"GetTimezone":             func() error { _, err := GetTimezone(ctx, database, 1); return err },
		"UpdateTimezone":          func() error { return UpdateTimezone(ctx, database, 1, "UTC") },
		"OfflineKeep":             func() error { _, err := OfflineKeep(ctx, database, 1); return err },
		"UpdateOfflineKeep":       func() error { return UpdateOfflineKeep(ctx, database, 1, 10) },
		"PixelTrackingEnabledAt":  func() error { _, err := PixelTrackingEnabledAt(ctx, database, 1); return err },
		"UpdatePixelTracking":     func() error { return UpdatePixelTracking(ctx, database, 1, true) },
		"GetPersonalOverview":     func() error { _, err := GetPersonalOverview(ctx, database, 1, "2025-01-01", "2025-02-01"); return err },
		"GetDailyReadCounts": func() error {
			_, err := GetDailyReadCounts(ctx, database, DailyReadQuery{UserID: 1, From: "2025-01-01", To: "2025-01-02", Loc: time.UTC})
			return err
		},
		"GetEntriesByCategory":    func() error { _, err := GetEntriesByCategory(ctx, database, 1, "2025-01-01", "2025-02-01"); return err },
		"GetTopFeeds":             func() error { _, err := GetTopFeeds(ctx, database, TopFeedsQuery{UserID: 1, Limit: 5}); return err },
		"GetAdminCounts":          func() error { _, err := GetAdminCounts(ctx, database); return err },
		"GetAdminEntryStats":      func() error { _, err := GetAdminEntryStats(ctx, database, "2025-01-01", "2025-02-01"); return err },
		"GetAdminDatabaseStats":   func() error { _, err := GetAdminDatabaseStats(ctx, database); return err },
		"FindEntrySummary":        func() error { _, err := FindEntrySummary(ctx, database, 1, 1); return err },
		"UpsertPendingSummary":    func() error { _, err := UpsertPendingSummary(ctx, database, 1, 1); return err },
		"SetSummaryProcessing":    func() error { return SetSummaryProcessing(ctx, database, 1, 1) },
		"SetSummaryCompleted":     func() error { _, err := SetSummaryCompleted(ctx, database, 1, 1, "t"); return err },
		"SetSummaryFailed":        func() error { _, err := SetSummaryFailed(ctx, database, 1, 1, "m"); return err },
		"DeleteEntrySummary":      func() error { _, err := DeleteEntrySummary(ctx, database, 1, 1); return err },
		"SummaryStatusesFor":      func() error { _, err := SummaryStatusesFor(ctx, database, 1, ids); return err },
		"FindIncompleteSummaries": func() error { _, err := FindIncompleteSummaries(ctx, database); return err },
		"DeleteExpiredSummaries":  func() error { _, err := DeleteExpiredSummaries(ctx, database, 24); return err },
		"CountCompletedSummaries": func() error { _, err := CountCompletedSummaries(ctx, database, 1); return err },
		"EntrySummaryExists":      func() error { _, err := EntrySummaryExists(ctx, database, 1, 1); return err },
	}
	for name, call := range reportsError {
		t.Run(name, func(t *testing.T) {
			if err := call(); err == nil {
				t.Error("want an error from a closed database, got nil")
			}
		})
	}
}
