package views

import "strconv"

// CredentialRow is what a session card and a GReader token card both show.
type CredentialRow struct {
	ID           int64
	UserAgent    string
	IPAddress    string
	CreatedAt    string
	CreatedAtISO string
	LastSeen     string
	LastSeenISO  string
	ExpiresAt    string
	ExpiresAtISO string
	// IsCurrent marks the session in use; always false for API tokens.
	IsCurrent bool
}

// AccountPage is the /user-settings page.
type AccountPage struct {
	Layout               *Layout
	CSRFToken            string
	Username             string
	Role                 string
	CreatedAt            string
	CreatedAtISO         string
	SessionCreatedAt     string
	SessionCreatedAtISO  string
	Sessions             []CredentialRow
	ShowSessions         bool
	APITokens            []CredentialRow
	PublicBaseURL        string
	Timezone             string
	TimezoneIsSuggestion bool
	TimezoneSuggestions  []string
	// Theme is "light", "dark" or empty for the system theme.
	Theme                     string
	EntriesPerPage            int64
	RetentionReadDays         int64
	EntriesPerPageSuggestions []int64
	RetentionDaysSuggestions  []int64
	OfflineKeepSuggestions    []int64
	OfflineKeep               int64
	SidebarSort               string
	SidebarHideRead           bool
	PixelTrackingEnabled      bool
	PasswordMinLength         int
	PasswordMaxLength         int
	LinkdingConfigured        bool
	LinkdingAPIURL            string
	KagiConfigured            bool
	KagiLanguage              string
	CredentialsUnreadable     bool
}

// Account renders the /user-settings page.
func Account(page AccountPage) string {
	layout := page.Layout
	body := el("div", attrs(class("app-layout")),
		el("rdrs-sidebar", attrs(a("active", "user-settings"))),
		el("main", attrs(class("main-content")),
			flashMount(layout.FlashMessages),
			el("div", attrs(class("page-content")),
				el("h1", nil, text("Settings")),
				accountInformation(page),
				show(page.ShowSessions, group(activeSessions(page), greaderTokens(page))),
				void("hr"),
				rssClient(page),
				void("hr"),
				changePassword(page),
				void("hr"),
				el("h2", nil, text("Passkeys")),
				el("p", attrs(class("muted")), text("Passkeys let you sign in without a password using your device's biometrics or security key.")),
				el("rdrs-passkeys", nil),
				void("hr"),
				displayPreferences(page),
				void("hr"),
				integrations(page))))
	return string(appPage(layout, "Settings", moduleScript(layout.GitVersion, "passkey.js"), body))
}

func localTime(iso, label string) node {
	return el("time", attrs(a("datetime", iso), flag("data-local-text")), text(label))
}

func accountInformation(page AccountPage) node {
	row := func(label string, value node) node {
		return el("tr", nil, el("th", attrs(class("settings-th")), text(label)), el("td", nil, value))
	}
	return group(el("h2", nil, text("Account Information")),
		el("table", nil,
			row("Username", text(page.Username)),
			row("Role", text(page.Role)),
			row("Registered", localTime(page.CreatedAtISO, page.CreatedAt)),
			row("Logged In", localTime(page.SessionCreatedAtISO, page.SessionCreatedAt))))
}

func credentialMeta(row CredentialRow) node {
	return el("dl", attrs(class("cred-card-meta")),
		el("div", attrs(class("cred-card-meta-wide")), el("dt", nil, text("User agent")),
			el("dd", attrs(class("cred-card-ua")), text(row.UserAgent))),
		el("div", nil, el("dt", nil, text("IP")), el("dd", nil, text(row.IPAddress))),
		el("div", nil, el("dt", nil, text("Signed in")), el("dd", nil, localTime(row.CreatedAtISO, row.CreatedAt))),
		el("div", nil, el("dt", nil, text("Last active")), el("dd", nil, localTime(row.LastSeenISO, row.LastSeen))),
		el("div", nil, el("dt", nil, text("Expires")), el("dd", nil, localTime(row.ExpiresAtISO, row.ExpiresAt))))
}

func revokeForm(action, csrfToken string) node {
	return el("form", attrs(a("method", "post"), a("action", action)),
		csrfField(csrfToken), text(" "),
		el("button", attrs(a("type", "submit"), class("btn btn-secondary")), text("Revoke")))
}

func sessionCard(row CredentialRow, csrfToken string) node {
	action := el("span", attrs(class("cred-card-current")), text("This device"))
	if !row.IsCurrent {
		action = revokeForm("/user-settings/sessions/"+strconv.FormatInt(row.ID, 10)+"/revoke", csrfToken)
	}
	return el("li", attrs(class("cred-card", onlyIf(row.IsCurrent, "cred-card--current"))),
		credentialMeta(row), el("div", attrs(class("cred-card-actions")), action))
}

func activeSessions(page AccountPage) node {
	return group(void("hr"), el("h2", nil, text("Active Sessions")),
		el("ul", attrs(class("cred-list"), a("data-testid", "sessions-list")),
			each(page.Sessions, func(row CredentialRow) node { return sessionCard(row, page.CSRFToken) })),
		el("form", attrs(a("method", "post"), a("action", "/user-settings/sessions/revoke-others")),
			csrfField(page.CSRFToken), text(" "),
			el("button", attrs(a("type", "submit")), text("Sign out other sessions"))))
}

func apiTokenCard(row CredentialRow, csrfToken string) node {
	return el("li", attrs(class("cred-card")), credentialMeta(row),
		el("div", attrs(class("cred-card-actions")),
			revokeForm("/user-settings/api-tokens/"+strconv.FormatInt(row.ID, 10)+"/revoke", csrfToken)))
}

func greaderTokens(page AccountPage) node {
	var content node
	if len(page.APITokens) == 0 {
		content = el("p", attrs(class("muted")), text("No GReader clients are currently signed in."))
	} else {
		content = group(
			el("ul", attrs(class("cred-list"), a("data-testid", "api-tokens-list")),
				each(page.APITokens, func(row CredentialRow) node { return apiTokenCard(row, page.CSRFToken) })),
			el("form", attrs(a("method", "post"), a("action", "/user-settings/api-tokens/revoke-all"),
				a("data-confirm", "Revoke all API tokens? Every connected GReader client will need to sign in again.")),
				csrfField(page.CSRFToken), text(" "),
				el("button", attrs(a("type", "submit"), class("btn btn-secondary")), text("Revoke all API tokens"))))
	}
	return group(void("hr"), el("h2", nil, text("GReader API Tokens")),
		el("p", attrs(class("muted")), text("These are Google Reader client tokens — issued to RSS apps (FeedMe, Read You, etc.) when they sign in below, kept separate from your browser sessions above. Revoking one forces that client to sign in again.")),
		content)
}

func rssClient(page AccountPage) node {
	row := func(label string, value node, valueAttrs ...attr) node {
		return el("tr", nil, el("th", attrs(class("settings-th")), text(label)), el("td", valueAttrs, value))
	}
	return group(el("h2", nil, text("RSS Client (Google Reader API)")),
		el("p", attrs(class("muted")), text("Connect any RSS reader that supports the Google Reader API (e.g., Reeder, NetNewsWire, FeedMe, Read You, FreshRSS).")),
		el("table", nil,
			row("Server URL", el("code", nil, text(page.PublicBaseURL))),
			row("Username", el("code", nil, text(page.Username))),
			row("Password", text("Your RDRS password"), class("muted"))),
		el("p", attrs(class("muted")),
			text("In your RSS client, choose \"Google Reader\" or \"FreshRSS\" as the account type and enter the server URL above with your credentials. Some clients may require the FreshRSS-compatible URL: "),
			el("code", nil, text(page.PublicBaseURL), text("/api/greader.php"))))
}

func passwordField(page AccountPage, id, name, label, autocomplete, testID string, hint node) node {
	return el("div", attrs(class("form-group")),
		el("label", attrs(a("for", id)), text(label)), text(" "),
		void("input", a("type", "password"), a("id", id), a("name", name), flag("required"),
			a("minlength", strconv.Itoa(page.PasswordMinLength)), a("maxlength", strconv.Itoa(page.PasswordMaxLength)),
			a("autocomplete", autocomplete), a("data-testid", testID)),
		hint)
}

func changePassword(page AccountPage) node {
	current := el("div", attrs(class("form-group")),
		el("label", attrs(a("for", "current-password")), text("Current Password")), text(" "),
		void("input", a("type", "password"), a("id", "current-password"), a("name", "current_password"), flag("required"),
			a("autocomplete", "current-password"), a("data-testid", "current-password")))
	hint := el("p", attrs(class("muted form-hint")),
		text("At least "+strconv.Itoa(page.PasswordMinLength)+" characters. A phrase of a few unrelated words works well — no mixture of cases or symbols is required."))
	return group(el("h2", nil, text("Change Password")),
		el("form", attrs(a("method", "post"), a("action", "/user-settings/password")),
			csrfField(page.CSRFToken), text(" "),
			current,
			passwordField(page, "new-password", "new_password", "New Password", "new-password", "new-password", hint),
			passwordField(page, "confirm-password", "confirm_password", "Confirm New Password", "new-password", "confirm-new-password", ""),
			el("button", attrs(a("type", "submit")), text("Change Password"))))
}

func suggestionList[T int64 | string](id string, values []T) node {
	return el("datalist", attrs(a("id", id)), each(values, func(value T) node {
		return el("option", attrs(a("value", suggestionText(value))))
	}))
}

func suggestionText[T int64 | string](value T) string {
	switch v := any(value).(type) {
	case int64:
		return strconv.FormatInt(v, 10)
	case string:
		return v
	}
	return ""
}

func displayPreferences(page AccountPage) node {
	return group(el("h2", nil, text("Display Preferences")),
		el("form", attrs(a("method", "post"), a("action", "/user-settings/preferences")),
			csrfField(page.CSRFToken), text(" "),
			timezoneField(page), themeField(page), entriesPerPageField(page), retentionField(page),
			sidebarFields(page), offlineKeepField(page), pixelTrackingField(page),
			el("button", attrs(a("type", "submit")), text("Save Preferences"))))
}

func timezoneField(page AccountPage) node {
	suggestion := show(page.TimezoneIsSuggestion, el("span",
		attrs(class("banner banner--warning"), a("role", "status"), a("data-testid", "timezone-suggestion")),
		text("Filled in from your browser's timezone. It is not saved yet: check it, then press Save.")))
	return el("div", attrs(class("form-group")),
		el("label", attrs(a("for", "timezone")), text("Timezone")), text(" "),
		void("input", a("type", "text"), a("id", "timezone"), a("name", "timezone"), a("value", page.Timezone),
			a("list", "timezone-list"), a("autocomplete", "off"), a("spellcheck", "false"), a("data-testid", "timezone"), flag("required")),
		suggestionList("timezone-list", page.TimezoneSuggestions),
		suggestion,
		el("span", attrs(class("muted text-xs")),
			text("An IANA name such as Asia/Taipei. Decides where "),
			el("a", attrs(a("href", "/statistics")), text("Statistics")),
			text(" cuts one day from the next; times elsewhere already show in your browser's timezone. Default UTC.")))
}

func themeField(page AccountPage) node {
	option := func(value, label string, selected bool) node {
		return el("option", attrs(a("value", value), when(selected, flag("selected"))), text(label))
	}
	return el("div", attrs(class("form-group")),
		el("label", attrs(a("for", "theme-select")), text("Theme")), text(" "),
		el("select", attrs(a("id", "theme-select"), a("name", "theme"), a("data-testid", "theme-select")),
			option("system", "System (auto)", page.Theme == ""),
			option("light", "Light", page.Theme == "light"),
			option("dark", "Dark", page.Theme == "dark")))
}

// numberField is a number input with a datalist of suggestions.
type numberField struct {
	id, name, label, testID string
	value                   int64
	min, max                string
	suggestions             []int64
	note                    node
}

func (field numberField) render() node {
	return el("div", attrs(class("form-group")),
		el("label", attrs(a("for", field.id)), text(field.label)), text(" "),
		void("input", a("type", "number"), a("id", field.id), a("name", field.name),
			a("value", strconv.FormatInt(field.value, 10)), a("min", field.min), a("max", field.max),
			a("list", field.id+"-list"), a("autocomplete", "off"), a("data-testid", field.testID), flag("required")),
		suggestionList(field.id+"-list", field.suggestions),
		field.note)
}

func mutedNote(content ...node) node {
	return el("span", attrs(class("muted text-xs")), content...)
}

func entriesPerPageField(page AccountPage) node {
	return numberField{"entries-per-page", "entries_per_page", "Entries per page", "entries-per-page",
		page.EntriesPerPage, "10", "100", page.EntriesPerPageSuggestions, mutedNote(text("(10-100)"))}.render()
}

func retentionField(page AccountPage) node {
	return numberField{"retention-read-days", "retention_read_days", "Delete read articles older than (days)", "retention-read-days",
		page.RetentionReadDays, "0", "3650", page.RetentionDaysSuggestions, mutedNote(text("(0 = never delete, max 3650)"))}.render()
}

func offlineKeepField(page AccountPage) node {
	note := mutedNote(
		text("(0 = off, max 200) Stores these articles in this browser so you can read them without a connection. Newest unread first, then starred. "),
		el("a", attrs(a("href", "/entries/offline"), a("data-testid", "offline-library-link")), text("See what is saved")),
		text("."))
	return numberField{"offline-keep", "offline_keep", "Keep offline (entries)", "offline-keep",
		page.OfflineKeep, "0", "200", page.OfflineKeepSuggestions, note}.render()
}

// checkboxField is a labelled checkbox with a muted explanation.
func checkboxField(id, name, label, testID string, checked bool, note node) node {
	return el("div", attrs(class("form-group")),
		el("label", nil, text(" "),
			void("input", a("type", "checkbox"), a("id", id), a("name", name), a("value", "1"),
				when(checked, flag("checked")), a("data-testid", testID)),
			text(" "+label+" ")),
		text(" "), note)
}

func sidebarFields(page AccountPage) node {
	option := func(value, label string) node {
		return el("option", attrs(a("value", value), when(page.SidebarSort == value, flag("selected"))), text(label))
	}
	order := el("div", attrs(class("form-group")),
		el("label", attrs(a("for", "sidebar-sort")), text("Sidebar order")), text(" "),
		el("select", attrs(a("id", "sidebar-sort"), a("name", "sidebar_sort"), a("data-testid", "sidebar-sort")),
			option("name", "Name (A-Z)"), option("unread", "Unread count (descending)")),
		text(" "), mutedNote(text("Applies to categories and to the open category's feeds.")))
	hideRead := checkboxField("sidebar-hide-read", "sidebar_hide_read", "Hide fully-read categories and feeds",
		"sidebar-hide-read", page.SidebarHideRead, mutedNote(text("The category or feed you are currently reading stays listed.")))
	return group(order, hideRead)
}

func pixelTrackingField(page AccountPage) node {
	note := mutedNote(
		text("Adds an invisible 1\u00d71 image to each entry's content, so opening one is recorded even when it happens in another reader app. Only this server sees the request \u2014 nothing leaves this instance. Shows up as an open rate on "),
		el("a", attrs(a("href", "/feeds")), text("Feeds")),
		text(" and "),
		el("a", attrs(a("href", "/statistics")), text("Statistics")),
		text(", which is a better guide to a low-value feed than the read count. "),
		el("strong", nil, text("Counts only entries that arrive after you switch this on")),
		text(", so the rate starts out empty and fills in as your feeds update \u2014 articles already in your library carry no image and are left out of the total."))
	return checkboxField("pixel-tracking", "pixel_tracking", "Track which entries get opened", "pixel-tracking",
		page.PixelTrackingEnabled, note)
}

func integrations(page AccountPage) node {
	warning := show(page.CredentialsUnreadable, credentialsUnreadableBanner())
	return group(el("h2", nil, text("Integrations")),
		el("p", attrs(class("muted")), text("Connect external services to save articles.")),
		warning, linkdingSection(page), kagiSection(page))
}

func credentialsUnreadableBanner() node {
	return el("div", attrs(class("banner banner--warning"), a("role", "alert"), a("data-testid", "credentials-unreadable")),
		el("span", attrs(class("banner-icon"), a("aria-hidden", "true"))),
		el("div", attrs(class("banner-body")),
			el("span", attrs(class("sr-only")), text("Warning: ")),
			text(" "),
			el("span", attrs(class("banner-message")), text("Your saved integration credentials cannot be decrypted with the current "),
				el("code", nil, text("RDRS_SECRET")),
				text(". They are still stored. Restore the previous secret to recover them, or re-enter them below to replace them."))))
}

func clearForm(action, confirm, label, csrfToken string) node {
	return el("form", attrs(a("method", "post"), a("action", action), class("form-inline-block"), a("data-confirm", confirm)),
		csrfField(csrfToken), text(" "),
		void("input", a("type", "hidden"), a("name", "_clear"), a("value", "1")), text(" "),
		el("button", attrs(a("type", "submit"), class("btn btn-secondary")), text(label)))
}

func serviceIntro(name, url, description string, configured bool) node {
	return el("p", attrs(class("muted")),
		el("a", attrs(a("href", url), a("target", "_blank"), a("rel", "noopener noreferrer")), text(name)),
		text(description),
		show(configured, el("span", attrs(class("success-text")), text("Configured"))))
}

func linkdingSection(page AccountPage) node {
	placeholder := "Enter your API token"
	if page.LinkdingConfigured {
		placeholder = "(unchanged)"
	}
	form := el("form", attrs(a("method", "post"), a("action", "/user-settings/linkding")),
		csrfField(page.CSRFToken), text(" "),
		el("div", attrs(class("form-group")),
			el("label", attrs(a("for", "linkding-api-url")), text("API URL")), text(" "),
			void("input", a("type", "url"), a("id", "linkding-api-url"), a("name", "api_url"), a("value", page.LinkdingAPIURL),
				a("placeholder", "https://linkding.example.com"))),
		el("div", attrs(class("form-group")),
			el("label", attrs(a("for", "linkding-api-token")), text("API Token")), text(" "),
			void("input", a("type", "password"), a("id", "linkding-api-token"), a("name", "api_token"), a("placeholder", placeholder))),
		el("button", attrs(a("type", "submit")), text("Save Linkding Settings")))
	return group(el("h3", nil, text("Linkding")),
		serviceIntro("Linkding", "https://github.com/sissbruecker/linkding", " is a self-hosted bookmark manager. ", page.LinkdingConfigured),
		form,
		show(page.LinkdingConfigured, clearForm("/user-settings/linkding", "Clear Linkding settings?", "Clear Linkding", page.CSRFToken)))
}

func kagiSection(page AccountPage) node {
	placeholder := "Paste your session link"
	if page.KagiConfigured {
		placeholder = "(unchanged)"
	}
	option := func(code, label string) node {
		return el("option", attrs(a("value", code), when(page.KagiLanguage == code, flag("selected"))), text(label))
	}
	languages := el("select", attrs(a("id", "kagi-language"), a("name", "language")),
		el("option", attrs(a("value", ""), when(page.KagiLanguage == "", flag("selected"))), text("Auto-detect")),
		option("EN", "English"), option("ZH-HANT", "繁體中文"), option("ZH-CN", "简体中文"), option("JA", "日本語"),
		option("KO", "한국어"), option("DE", "Deutsch"), option("FR", "Français"), option("ES", "Español"), option("PT", "Português"))
	form := el("form", attrs(a("method", "post"), a("action", "/user-settings/kagi")),
		csrfField(page.CSRFToken), text(" "),
		el("div", attrs(class("form-group")),
			el("label", attrs(a("for", "kagi-session-link")), text("Session Link")), text(" "),
			void("input", a("type", "text"), a("id", "kagi-session-link"), a("name", "session_link"), a("placeholder", placeholder))),
		el("div", attrs(class("form-group")),
			el("label", attrs(a("for", "kagi-language")), text("Target Language")), text(" "), languages),
		el("button", attrs(a("type", "submit")), text("Save Kagi Settings")))
	return group(el("h3", nil, text("Kagi Universal Summarizer")),
		serviceIntro("Kagi Universal Summarizer", "https://kagi.com/summarizer", " provides AI-powered article summaries. ", page.KagiConfigured),
		form,
		show(page.KagiConfigured, clearForm("/user-settings/kagi", "Clear Kagi settings?", "Clear Kagi", page.CSRFToken)))
}
