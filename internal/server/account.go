package server

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"net/netip"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/datetime"
	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/save"
	"github.com/henry40408/rdrs/internal/timezone"
	"github.com/henry40408/rdrs/internal/views"
)

const userSettingsLocation = "/user-settings"

func (a *App) accountRoutes() []Route {
	return []Route{
		{"GET", "/user-settings", a.userSettingsPage},
		{"GET", "/settings", a.serverSettingsPage},
		{"POST", "/user-settings/password", a.changePasswordForm},
		{"POST", "/user-settings/preferences", a.updatePreferencesForm},
		{"POST", "/user-settings/linkding", a.updateLinkdingForm},
		{"POST", "/user-settings/kagi", a.updateKagiForm},
		{"POST", "/user-settings/sessions/revoke-others", a.revokeOtherSessionsForm},
		{"POST", "/user-settings/sessions/{id}/revoke", a.revokeSessionForm},
		{"POST", "/user-settings/api-tokens/{id}/revoke", a.revokeAPITokenForm},
		{"POST", "/user-settings/api-tokens/revoke-all", a.revokeAllAPITokensForm},
		{"GET", "/api/passkeys", a.listPasskeys},
		{"PUT", "/api/passkeys/{id}", a.renamePasskey},
		{"DELETE", "/api/passkeys/{id}", a.deletePasskey},
		{"PUT", "/api/user/client-info", a.putClientInfo},
	}
}

func settingsFlash(w http.ResponseWriter, message flash.Message) {
	flash.Redirect(w, userSettingsLocation, message)
}

// --- pages ---

// serverSettingsPage is GET /settings: the running configuration, read-only.
func (a *App) serverSettingsPage(w http.ResponseWriter, r *http.Request) {
	// Admin-only: the page exposes deployment internals.
	principal := a.Auth.AdminPage(r)
	if principal == nil {
		flash.Redirect(w, "/login", flash.NewWarning("Please log in to continue."))
		return
	}
	layout := a.buildLayout(r, principal, flash.Take(w, r))
	cfg := a.Config
	writeHTML(w, http.StatusOK, views.ServerSettings(views.ServerSettingsPage{
		Layout:                &layout,
		DatabaseURL:           config.RedactDatabaseURL(cfg.DatabaseURL),
		ServerBind:            cfg.ServerBind.String(),
		UserAgent:             cfg.UserAgent,
		UserAgentIsDefault:    cfg.UserAgent == config.DefaultUserAgent(),
		MultiUserEnabled:      cfg.MultiUser,
		SecretGenerated:       cfg.SecretGenerated,
		WebAuthnRPID:          cfg.WebAuthnRPID,
		WebAuthnRPOrigin:      cfg.WebAuthnRPOrigin,
		WebAuthnRPName:        cfg.WebAuthnRPName,
		AuthProxyHeader:       cfg.AuthProxyHeader,
		TrustedProxyNetworks:  joinPrefixes(cfg.TrustedProxyNetworks),
		AuthProxyUserCreation: cfg.AuthProxyUserCreation,
		AuthProxyGroupsHeader: cfg.AuthProxyGroupsHeader,
		AuthProxyAdminGroup:   cfg.AuthProxyAdminGroup,
		DisableLocalAuth:      cfg.DisableLocalAuth,
		AuthProxyLogoutURL:    cfg.AuthProxyLogoutURL,
	}))
}

// userSettingsPage is GET /user-settings.
func (a *App) userSettingsPage(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return
	}
	ctx := r.Context()
	userID := principal.User.ID
	layout := a.buildLayout(r, principal, flash.Take(w, r))
	settings, err := models.LoadUserSettings(ctx, a.DB, userID)
	if err != nil {
		settings = models.DefaultUserSettings(userID)
	}
	stored, _ := models.SaveServicesOf(ctx, a.DB, userID, a.serviceKey())
	services := save.ConfigOf(stored)
	prefs := settings.SidebarPrefs()
	zone, suggested := suggestedTimezone(r, settings.Tz())
	masquerading := principal.Session.IsMasquerading()

	page := views.AccountPage{
		Layout:              &layout,
		CSRFToken:           principal.CSRFToken,
		Username:            principal.User.Username,
		Role:                string(principal.User.Role),
		CreatedAt:           datetime.FormatDisplay(principal.User.CreatedAt),
		CreatedAtISO:        chronoRFC3339(principal.User.CreatedAt),
		SessionCreatedAt:    datetime.FormatDisplay(principal.Session.CreatedAt),
		SessionCreatedAtISO: chronoRFC3339(principal.Session.CreatedAt),
		// Both lists stay hidden while masquerading: never expose the
		// target's sessions or tokens.
		ShowSessions:              !masquerading,
		PublicBaseURL:             a.publicBaseURL(),
		Timezone:                  zone.String(),
		TimezoneIsSuggestion:      suggested,
		TimezoneSuggestions:       timezone.Suggestions,
		EntriesPerPage:            settings.EntriesPerPage,
		RetentionReadDays:         settings.RetentionReadDays,
		EntriesPerPageSuggestions: models.EntriesPerPageSuggestions,
		RetentionDaysSuggestions:  models.RetentionReadDaysSuggestions,
		OfflineKeepSuggestions:    models.OfflineKeepSuggestions,
		OfflineKeep:               settings.OfflineKeep,
		SidebarSort:               prefs.Sort,
		SidebarHideRead:           prefs.HideRead,
		PixelTrackingEnabled:      settings.PixelTrackingSince != nil,
		PasswordMinLength:         auth.PasswordMinLength,
		PasswordMaxLength:         auth.PasswordMaxLength,
		CredentialsUnreadable:     stored.Undecryptable,
	}
	if settings.Theme != nil {
		page.Theme = *settings.Theme
	}
	if services.Linkding != nil {
		page.LinkdingConfigured = services.Linkding.IsConfigured()
		page.LinkdingAPIURL = services.Linkding.APIURL
	}
	if services.Kagi != nil {
		page.KagiConfigured = services.Kagi.IsConfigured()
		if services.Kagi.Language != nil {
			page.KagiLanguage = *services.Kagi.Language
		}
	}
	if !masquerading {
		page.Sessions = a.sessionRows(ctx, userID, principal.Session.ID)
		page.APITokens = a.apiTokenRows(ctx, userID)
	}
	writeHTML(w, http.StatusOK, views.Account(page))
}

// serviceKey is the key sealing service credentials, nil when RDRS_SECRET was
// generated (so the credentials stay plaintext rather than lost on restart).
func (a *App) serviceKey() []byte {
	key, _ := a.Config.ServiceTokenKey()
	return key
}

func (a *App) publicBaseURL() string {
	if a.Config.PublicBaseURL != "" {
		return a.Config.PublicBaseURL
	}
	return "http://localhost:" + strconv.Itoa(int(a.Config.ServerBind.Port()))
}

// suggestedTimezone is the ?timezone= value of the browser-timezone notice,
// when it names a zone that cuts days differently from the stored one. Anything
// else is ignored, so a mangled link still opens the page.
func suggestedTimezone(r *http.Request, stored *time.Location) (*time.Location, bool) {
	values := r.URL.Query()["timezone"]
	if len(values) == 0 {
		return stored, false
	}
	suggested, ok := timezone.Parse(strings.TrimSpace(values[0]))
	if !ok || timezone.SameRules(suggested, stored, time.Now()) {
		return stored, false
	}
	return suggested, true
}

func (a *App) sessionRows(ctx context.Context, userID, currentID int64) []views.CredentialRow {
	sessions, err := models.ListUserSessions(ctx, a.DB, userID)
	if err != nil {
		return nil
	}
	rows := []views.CredentialRow{}
	for _, s := range sessions {
		if s.IsExpired() {
			continue
		}
		rows = append(rows, views.CredentialRow{
			ID: s.ID, UserAgent: s.UserAgent, IPAddress: s.IPAddress, IsCurrent: s.ID == currentID,
			CreatedAt: datetime.FormatDisplay(s.CreatedAt), CreatedAtISO: chronoRFC3339(s.CreatedAt),
			LastSeen: datetime.FormatDisplay(s.LastSeenAt), LastSeenISO: chronoRFC3339(s.LastSeenAt),
			ExpiresAt: datetime.FormatDisplay(s.ExpiresAt), ExpiresAtISO: chronoRFC3339(s.ExpiresAt),
		})
	}
	return rows
}

func (a *App) apiTokenRows(ctx context.Context, userID int64) []views.CredentialRow {
	tokens, err := models.ListUserTokens(ctx, a.DB, userID)
	if err != nil {
		return nil
	}
	rows := []views.CredentialRow{}
	for _, t := range tokens {
		if t.IsExpired() {
			continue
		}
		rows = append(rows, views.CredentialRow{
			ID: t.ID, UserAgent: t.UserAgent, IPAddress: t.IPAddress,
			CreatedAt: datetime.FormatDisplay(t.CreatedAt), CreatedAtISO: chronoRFC3339(t.CreatedAt),
			LastSeen: datetime.FormatDisplay(t.LastSeenAt), LastSeenISO: chronoRFC3339(t.LastSeenAt),
			ExpiresAt: datetime.FormatDisplay(t.ExpiresAt), ExpiresAtISO: chronoRFC3339(t.ExpiresAt),
		})
	}
	return rows
}

// --- password ---

// changePasswordForm is POST /user-settings/password. Success signs the user
// out everywhere (sessions and API tokens), so it ends on the login page.
func (a *App) changePasswordForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok || !requireFields(w, form, "current_password", "new_password", "confirm_password") {
		return
	}
	newPassword := form.Get("new_password")
	if newPassword != form.Get("confirm_password") {
		settingsFlash(w, flash.NewError("New passwords do not match."))
		return
	}
	ip := a.clientIP(r)
	// After the free mismatch check, before the costly hash work.
	if decision := a.Limiter.TryAcquire(auth.BucketPasswordChange, ip); !decision.Allowed {
		a.logRateLimited(ip.String(), "password_change", "POST /user-settings/password", "password_change")
		settingsFlash(w, flash.NewError(tooManyAttempts(decision.RetryAfterSecs)))
		return
	}
	if message := passwordRefusal(principal, form.Get("current_password"), newPassword); message != "" {
		// A refusal keeps its rate-limit reservation.
		settingsFlash(w, flash.NewError(message))
		return
	}
	a.Limiter.Release(auth.BucketPasswordChange, ip)
	a.storeNewPassword(w, r, principal, newPassword)
}

// passwordRefusal is the message for a rejected change, or "" when allowed.
func passwordRefusal(principal *auth.Principal, current, next string) string {
	var validation *models.ValidationError
	if err := auth.ValidatePasswordStrength(next, []string{principal.User.Username}); errors.As(err, &validation) {
		if strings.HasSuffix(validation.Message, ".") {
			return validation.Message
		}
		return validation.Message + "."
	}
	if !auth.VerifyPassword(current, principal.User.PasswordHash) {
		return "Current password is incorrect."
	}
	return ""
}

func (a *App) storeNewPassword(w http.ResponseWriter, r *http.Request, principal *auth.Principal, password string) {
	hash, err := auth.HashPassword(password)
	if err != nil {
		settingsFlash(w, flash.NewError("Failed to hash password."))
		return
	}
	userID := principal.User.ID
	if err := models.ChangePassword(r.Context(), a.DB, userID, hash); err != nil {
		settingsFlash(w, flash.NewError("Failed to update password."))
		return
	}
	a.Audit.SessionsDestroyedBulk(userID, "password_change", nil)
	a.Audit.APITokensDestroyed(userID, "password_change", nil)
	flash.Redirect(w, "/login", flash.NewSuccess("Password changed successfully. Please login with your new password."))
}

// --- sessions and tokens ---

const (
	masqueradeBlocksSessions = "Session management is unavailable while masquerading."
	masqueradeBlocksServices = "Service credentials cannot be changed while masquerading."
)

// ErrMasqueradeBlocksPasskeys guards the JSON passkey routes: a passkey added
// while masquerading would still sign in as the target afterwards.
var ErrMasqueradeBlocksPasskeys = newError(http.StatusForbidden, "Passkeys cannot be managed while masquerading.")

// sessionForm authenticates a credential-management form. It reports false
// after answering; masquerading is refused so the target's credentials stay
// out of reach.
func (a *App) sessionForm(w http.ResponseWriter, r *http.Request) (*auth.Principal, bool) {
	return a.formUnlessMasquerading(w, r, masqueradeBlocksSessions)
}

// serviceForm is sessionForm for the stored service credentials: redirecting a
// target's api_url would send its stored token to the admin's host.
func (a *App) serviceForm(w http.ResponseWriter, r *http.Request) (*auth.Principal, bool) {
	return a.formUnlessMasquerading(w, r, masqueradeBlocksServices)
}

func (a *App) formUnlessMasquerading(w http.ResponseWriter, r *http.Request, refusal string) (*auth.Principal, bool) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return nil, false
	}
	if principal.Session.IsMasquerading() {
		settingsFlash(w, flash.NewError(refusal))
		return nil, false
	}
	return principal, true
}

func countNoun(count int64, noun string) string {
	if count == 1 {
		return noun
	}
	return noun + "s"
}

// revokeOtherSessionsForm signs out other browser sessions; API tokens stay.
func (a *App) revokeOtherSessionsForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.sessionForm(w, r)
	if !ok {
		return
	}
	userID := principal.User.ID
	count, err := models.DeleteUserSessionsExcept(r.Context(), a.DB, userID, principal.Session.SessionToken)
	switch {
	case err != nil:
		settingsFlash(w, flash.NewError("Failed to sign out other sessions."))
	case count == 0:
		zero := uint64(0)
		a.Audit.SessionsDestroyedBulk(userID, "revoke_others", &zero)
		settingsFlash(w, flash.NewInfo("No other sessions were signed in — nothing to sign out."))
	default:
		destroyed := uint64(count)
		a.Audit.SessionsDestroyedBulk(userID, "revoke_others", &destroyed)
		settingsFlash(w, flash.NewSuccess("Signed out "+strconv.FormatInt(count, 10)+" other "+countNoun(count, "session")+"."))
	}
}

// revokeSessionForm revokes one session. The current one is refused here, not
// just hidden in the page.
func (a *App) revokeSessionForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.sessionForm(w, r)
	if !ok {
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	if id == principal.Session.ID {
		settingsFlash(w, flash.NewError("That is the session you are using — sign out instead."))
		return
	}
	userID := principal.User.ID
	count, err := models.DeleteUserSessionByID(r.Context(), a.DB, id, userID)
	switch {
	case err != nil:
		settingsFlash(w, flash.NewError("Failed to revoke session."))
	case count == 0:
		settingsFlash(w, flash.NewError("That session is no longer active."))
	default:
		destroyed := uint64(count)
		a.Audit.SessionsDestroyedBulk(userID, "revoke_session", &destroyed)
		settingsFlash(w, flash.NewSuccess("Session revoked."))
	}
}

// revokeAPITokenForm revokes one GReader token of the user.
func (a *App) revokeAPITokenForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.sessionForm(w, r)
	if !ok {
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	userID := principal.User.ID
	if err := models.DeleteToken(r.Context(), a.DB, id, userID); err != nil {
		settingsFlash(w, flash.NewError("Failed to revoke API token."))
		return
	}
	a.Audit.APITokensDestroyed(userID, "revoke_token", nil)
	settingsFlash(w, flash.NewSuccess("API token revoked."))
}

// revokeAllAPITokensForm revokes every GReader token of the user.
func (a *App) revokeAllAPITokensForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.sessionForm(w, r)
	if !ok {
		return
	}
	userID := principal.User.ID
	count, err := models.DeleteUserTokens(r.Context(), a.DB, userID)
	destroyed := uint64(count)
	switch {
	case err != nil:
		settingsFlash(w, flash.NewError("Failed to revoke API tokens."))
	case count == 0:
		a.Audit.APITokensDestroyed(userID, "revoke_all", &destroyed)
		settingsFlash(w, flash.NewInfo("There were no API tokens to revoke."))
	default:
		a.Audit.APITokensDestroyed(userID, "revoke_all", &destroyed)
		settingsFlash(w, flash.NewSuccess("Revoked "+strconv.FormatInt(count, 10)+" API "+countNoun(count, "token")+"."))
	}
}

// --- preferences ---

// preferencesInput is the parsed preferences form; nil fields were absent and
// leave the stored value unchanged.
type preferencesInput struct {
	theme           *string
	entriesPerPage  int64
	retentionDays   int64
	sidebarSort     string
	sidebarHideRead bool
	offlineKeep     *int64
	pixelTracking   bool
	timezone        *string
}

func readPreferences(w http.ResponseWriter, form url.Values) (preferencesInput, bool) {
	reader := newFormReader(form)
	var input preferencesInput
	theme := reader.text("theme")
	entries, retention := reader.id("entries_per_page"), reader.id("retention_read_days")
	sort := reader.text("sidebar_sort")
	hideRead, pixel := reader.text("sidebar_hide_read"), reader.text("pixel_tracking")
	input.offlineKeep, input.timezone = reader.id("offline_keep"), reader.text("timezone")
	for name, value := range map[string]any{"theme": theme, "entries_per_page": entries, "retention_read_days": retention} {
		if reader.err == nil && isNilPointer(value) {
			reader.err = errors.New("missing field `" + name + "`")
		}
	}
	if reader.failed(w) {
		return input, false
	}
	if *theme == "light" || *theme == "dark" {
		input.theme = theme
	}
	input.entriesPerPage, input.retentionDays = *entries, *retention
	input.sidebarSort = models.DefaultSidebarPrefs().Sort
	if sort != nil {
		input.sidebarSort = *sort
	}
	input.sidebarHideRead, input.pixelTracking = hideRead != nil, pixel != nil
	return input, true
}

func isNilPointer(value any) bool {
	switch typed := value.(type) {
	case *string:
		return typed == nil
	case *int64:
		return typed == nil
	}
	return false
}

// updatePreferencesForm is POST /user-settings/preferences.
func (a *App) updatePreferencesForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok {
		return
	}
	input, ok := readPreferences(w, form)
	if !ok {
		return
	}
	// Checked up front: a refusal after the first write would leave the form
	// half saved and the sidebar cache stale.
	if input.timezone != nil {
		if _, valid := timezone.Parse(strings.TrimSpace(*input.timezone)); !valid {
			settingsFlash(w, flash.NewError("\""+strings.TrimSpace(*input.timezone)+"\" is not a timezone name; use one such as Asia/Taipei"))
			return
		}
	}
	userID := principal.User.ID
	if err := a.savePreferences(r.Context(), userID, input); err != nil {
		var validation *models.ValidationError
		if errors.As(err, &validation) {
			settingsFlash(w, flash.NewError(validation.Message))
			return
		}
		slog.Error("preferences update failed", "event", "settings.preferences_failed", "error", err)
		settingsFlash(w, flash.NewError("Failed to update preferences."))
		return
	}
	a.Sidebar.Bust(userID)
	settingsFlash(w, flash.NewSuccess("Preferences updated."))
}

func (a *App) savePreferences(ctx context.Context, userID int64, input preferencesInput) error {
	steps := []func() error{
		func() error { _, err := models.UpsertUserSettings(ctx, a.DB, userID, input.entriesPerPage); return err },
		func() error { return models.UpdateTheme(ctx, a.DB, userID, input.theme) },
		func() error { return models.UpdateRetentionReadDays(ctx, a.DB, userID, input.retentionDays) },
		func() error {
			return models.UpdateSidebarPrefs(ctx, a.DB, userID,
				models.SidebarPrefs{Sort: input.sidebarSort, HideRead: input.sidebarHideRead})
		},
		func() error {
			if input.offlineKeep == nil {
				return nil
			}
			return models.UpdateOfflineKeep(ctx, a.DB, userID, *input.offlineKeep)
		},
		func() error {
			if input.timezone == nil {
				return nil
			}
			return models.UpdateTimezone(ctx, a.DB, userID, *input.timezone)
		},
		func() error { return models.UpdatePixelTracking(ctx, a.DB, userID, input.pixelTracking) },
	}
	for _, step := range steps {
		if err := step(); err != nil {
			return err
		}
	}
	return nil
}

// --- service credentials ---

// loadServicesForWrite reads the stored services to edit. An unreadable value
// reads as empty: re-submitting the form is how the user recovers from a
// rotated RDRS_SECRET.
func (a *App) loadServicesForWrite(ctx context.Context, userID int64) (save.ServicesConfig, error) {
	stored, err := models.SaveServicesOf(ctx, a.DB, userID, a.serviceKey())
	if err != nil {
		return save.ServicesConfig{}, err
	}
	return save.ConfigOf(stored), nil
}

func (a *App) storeServices(ctx context.Context, userID int64, services save.ServicesConfig) error {
	encoded, err := services.JSON()
	if err != nil {
		return err
	}
	_, err = models.UpdateSaveServices(ctx, a.DB, userID, encoded, a.serviceKey())
	return err
}

// optionalText is a form field that counts as absent when empty.
func optionalText(form url.Values, name string) *string {
	value, present := form[name]
	if !present || value[0] == "" {
		return nil
	}
	return &value[0]
}

// updateLinkdingForm is POST /user-settings/linkding.
func (a *App) updateLinkdingForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.serviceForm(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok {
		return
	}
	_, clear := form["_clear"]
	userID := principal.User.ID
	err := a.editServices(r.Context(), userID, func(services *save.ServicesConfig) {
		services.Linkding = nextLinkding(services.Linkding, form, clear)
	})
	switch {
	case err == nil && clear:
		settingsFlash(w, flash.NewSuccess("Linkding configuration cleared."))
	case err == nil:
		settingsFlash(w, flash.NewSuccess("Linkding configuration updated."))
	default:
		settingsFlash(w, flash.NewError("Failed to update Linkding configuration."))
	}
}

func (a *App) editServices(ctx context.Context, userID int64, edit func(*save.ServicesConfig)) error {
	services, err := a.loadServicesForWrite(ctx, userID)
	if err != nil {
		return err
	}
	edit(&services)
	return a.storeServices(ctx, userID, services)
}

// nextLinkding merges the submitted fields into the stored config; an empty
// submission removes it, and a blank field keeps its stored value.
func nextLinkding(current *save.LinkdingConfig, form url.Values, clear bool) *save.LinkdingConfig {
	apiURL, apiToken := optionalText(form, "api_url"), optionalText(form, "api_token")
	if clear || (apiURL == nil && apiToken == nil) {
		return nil
	}
	next := save.LinkdingConfig{}
	if current != nil {
		next = *current
	}
	if apiURL != nil {
		next.APIURL = *apiURL
	}
	if apiToken != nil {
		next.APIToken = *apiToken
	}
	return &next
}

// kagiSessionToken extracts the token query parameter of a Kagi session link.
func kagiSessionToken(link string) (string, error) {
	parsed, err := url.Parse(strings.TrimSpace(link))
	if err != nil || !parsed.IsAbs() {
		return "", errors.New("Invalid session link URL")
	}
	token := parsed.Query().Get("token")
	if token == "" {
		return "", errors.New("No token found in session link")
	}
	return token, nil
}

// updateKagiForm is POST /user-settings/kagi.
func (a *App) updateKagiForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.serviceForm(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok {
		return
	}
	userID := principal.User.ID
	if _, clear := form["_clear"]; clear {
		a.clearKagi(w, r, userID)
		return
	}
	var token *string
	if link := optionalText(form, "session_link"); link != nil {
		parsed, err := kagiSessionToken(*link)
		if err != nil {
			settingsFlash(w, flash.NewError(err.Error()))
			return
		}
		token = &parsed
	}
	_, hasLanguage := form["language"]
	language := optionalText(form, "language")
	err := a.editServices(r.Context(), userID, func(services *save.ServicesConfig) {
		services.Kagi = nextKagi(services.Kagi, kagiEdit{token: token, hasLanguage: hasLanguage, language: language})
	})
	if err != nil {
		settingsFlash(w, flash.NewError("Failed to update Kagi configuration."))
		return
	}
	settingsFlash(w, flash.NewSuccess("Kagi configuration updated."))
}

func (a *App) clearKagi(w http.ResponseWriter, r *http.Request, userID int64) {
	err := a.editServices(r.Context(), userID, func(services *save.ServicesConfig) { services.Kagi = nil })
	if err != nil {
		settingsFlash(w, flash.NewError("Failed to clear Kagi configuration."))
		return
	}
	settingsFlash(w, flash.NewSuccess("Kagi configuration cleared."))
}

// kagiEdit is what the Kagi form submitted.
type kagiEdit struct {
	token       *string
	hasLanguage bool
	language    *string
}

// nextKagi applies a submission: a missing token keeps the stored one, a
// missing language field keeps the stored language, and an empty form removes
// the config.
func nextKagi(current *save.KagiConfig, edit kagiEdit) *save.KagiConfig {
	if edit.token == nil && !edit.hasLanguage {
		return nil
	}
	next := save.KagiConfig{}
	if current != nil {
		next = *current
	}
	if edit.token != nil {
		next.SessionToken = *edit.token
	}
	if edit.hasLanguage {
		next.Language = edit.language
	}
	return &next
}

// --- passkeys and client info ---

type passkeyView struct {
	ID         int64   `json:"id"`
	Name       string  `json:"name"`
	CreatedAt  string  `json:"created_at"`
	LastUsedAt *string `json:"last_used_at"`
}

// listPasskeys is GET /api/passkeys. Credential ids and keys never leave the
// server.
func (a *App) listPasskeys(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	passkeys, err := models.ListUserPasskeys(r.Context(), a.DB, principal.User.ID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	views := make([]passkeyView, 0, len(passkeys))
	for _, p := range passkeys {
		view := passkeyView{ID: p.ID, Name: p.Name, CreatedAt: datetime.FormatDisplay(p.CreatedAt)}
		if p.LastUsedAt != nil {
			used := datetime.FormatDisplay(*p.LastUsedAt)
			view.LastUsedAt = &used
		}
		views = append(views, view)
	}
	writeJSON(w, http.StatusOK, map[string]any{"passkeys": views})
}

func passkeyError(err error) error {
	if errors.Is(err, models.ErrPasskeyNotFound) {
		return ErrPasskeyNotFound
	}
	return DatabaseError(err)
}

// renamePasskey is PUT /api/passkeys/{id}.
func (a *App) renamePasskey(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	if principal.Session.IsMasquerading() {
		writeError(w, ErrMasqueradeBlocksPasskeys)
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	var body struct {
		Name *string `json:"name"`
	}
	if !decodeJSON(w, r, &body) {
		return
	}
	if body.Name == nil {
		rejection(w, http.StatusUnprocessableEntity, "Failed to deserialize the JSON body into the target type: missing field `name`")
		return
	}
	if *body.Name == "" {
		writeError(w, ValidationError("Name is required"))
		return
	}
	if err := models.RenamePasskey(r.Context(), a.DB, id, principal.User.ID, *body.Name); err != nil {
		writeError(w, passkeyError(err))
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// deletePasskey is DELETE /api/passkeys/{id}; it needs a recent password
// confirmation, because removing the last passkey can lock the user out.
func (a *App) deletePasskey(w http.ResponseWriter, r *http.Request) {
	principal, err := a.Auth.RecentlyAuthenticated(r)
	if err != nil {
		writeError(w, authError(err))
		return
	}
	if principal.Session.IsMasquerading() {
		writeError(w, ErrMasqueradeBlocksPasskeys)
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	if err := models.DeletePasskey(r.Context(), a.DB, id, principal.User.ID); err != nil {
		writeError(w, passkeyError(err))
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

type clientHint struct {
	Message     string `json:"message"`
	ActionLabel string `json:"action_label"`
	ActionHref  string `json:"action_href"`
}

// putClientInfo is PUT /api/user/client-info: facts only the browser knows.
// Nothing is stored; each fact is compared with the settings and answered
// with the hints the page should show.
func (a *App) putClientInfo(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	var info struct {
		Timezone *string `json:"timezone"`
	}
	if !decodeJSON(w, r, &info) {
		return
	}
	hints := []clientHint{}
	// The browser belongs to the admin, not to the account being viewed.
	if info.Timezone != nil && !principal.Session.IsMasquerading() {
		hint, err := a.timezoneHint(r.Context(), principal.User.ID, *info.Timezone)
		if err != nil {
			writeError(w, appError(err))
			return
		}
		if hint != nil {
			hints = append(hints, *hint)
		}
	}
	writeJSON(w, http.StatusOK, map[string]any{"hints": hints})
}

// timezoneHint suggests the browser's timezone when it cuts days differently
// from the stored one. It only suggests: the settings form applies the change.
func (a *App) timezoneHint(ctx context.Context, userID int64, name string) (*clientHint, error) {
	name = strings.TrimSpace(name)
	browser, ok := timezone.Parse(name)
	if !ok {
		return nil, &models.ValidationError{Message: "\"" + name + "\" is not a timezone name"}
	}
	stored, err := models.GetTimezone(ctx, a.DB, userID)
	if err != nil {
		return nil, err
	}
	if timezone.SameRules(browser, stored, time.Now()) {
		return nil, nil
	}
	return &clientHint{
		Message: "Your browser's timezone is " + browser.String() +
			", but Statistics cuts days in " + stored.String() + ".",
		ActionLabel: "Review in Preferences",
		ActionHref:  "/user-settings?timezone=" + url.QueryEscape(browser.String()),
	}, nil
}

func joinPrefixes(prefixes []netip.Prefix) string {
	names := make([]string, len(prefixes))
	for i, prefix := range prefixes {
		names[i] = prefix.String()
	}
	return strings.Join(names, ", ")
}
