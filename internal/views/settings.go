package views

// ServerSettingsPage is the /settings table of the running configuration.
type ServerSettingsPage struct {
	Layout                *Layout
	DatabaseURL           string
	ServerBind            string
	UserAgent             string
	UserAgentIsDefault    bool
	MultiUserEnabled      bool
	SecretGenerated       bool
	WebAuthnRPID          string
	WebAuthnRPOrigin      string
	WebAuthnRPName        string
	AuthProxyHeader       string
	TrustedProxyNetworks  string
	AuthProxyUserCreation bool
	AuthProxyGroupsHeader string
	AuthProxyAdminGroup   string
	DisableLocalAuth      bool
	AuthProxyLogoutURL    string
}

// ServerSettings renders the /settings page.
func ServerSettings(page ServerSettingsPage) string {
	layout := page.Layout
	table := el("table", attrs(class("mobile-cards-settings")),
		el("thead", nil, el("tr", nil,
			el("th", attrs(class("settings-th")), text("Variable")),
			el("th", attrs(class("settings-th settings-col-current")), text("Current")),
			el("th", attrs(class("settings-th")), text("Default")),
			el("th", attrs(class("settings-th")), text("Description")))),
		el("tbody", nil,
			serverRows(page), accountRows(page), passkeyRows(page), forwardAuthRows(page), contentRows(page)))
	body := el("div", attrs(class("app-layout")),
		el("rdrs-sidebar", attrs(a("active", "settings"))),
		el("main", attrs(class("main-content")),
			flashMount(layout.FlashMessages),
			el("div", attrs(class("page-content")),
				el("h1", nil, text("App")),
				el("div", attrs(a("id", "settings-body")),
					el("p", attrs(class("muted")), text("Version: "), el("code", nil, text(layout.GitVersion))),
					el("h2", nil, text("Configuration")),
					el("p", attrs(class("muted")), text("These settings are configured via environment variables and cannot be changed at runtime. The current column reflects the running instance.")),
					table))))
	return string(appPage(layout, "App", "", body))
}

func settingsGroupHeader(name string) node {
	return el("tr", attrs(class("grouphdr")), el("td", attrs(a("colspan", "4")), text(name)))
}

func yesNo(value bool) node {
	if value {
		return el("span", attrs(class("success-text")), text("Yes"))
	}
	return el("span", attrs(class("muted")), text("No"))
}

func unsetDefault() node { return el("em", nil, text("(unset)")) }

func codeDefault(value string) node { return el("code", nil, text(value)) }

// settingRow is one variable: its current value, default and description.
type settingRow struct {
	variable             string
	current, def, detail node
}

func (row settingRow) render() node {
	return el("tr", nil,
		el("td", nil, el("code", nil, text(row.variable))),
		el("td", attrs(class("settings-col-current"), a("data-label", "Current")), row.current),
		el("td", attrs(a("data-label", "Default")), row.def),
		el("td", attrs(a("data-label", "Description")), row.detail))
}

func settingRows(header string, rows ...settingRow) node {
	rendered := []node{settingsGroupHeader(header)}
	for _, row := range rows {
		rendered = append(rendered, row.render())
	}
	return group(rendered...)
}

func serverRows(page ServerSettingsPage) node {
	return settingRows("Server",
		settingRow{"DATABASE_URL", codeDefault(page.DatabaseURL), codeDefault("rdrs.sqlite3"), text("SQLite database file path")},
		settingRow{"RDRS_SERVER_BIND", codeDefault(page.ServerBind), codeDefault("127.0.0.1:8080"), text("HTTP server bind address (host:port)")})
}

func accountRows(page ServerSettingsPage) node {
	return settingRows("Accounts",
		settingRow{"RDRS_MULTI_USER_ENABLED", yesNo(page.MultiUserEnabled), codeDefault("false"), text("Allow multiple users")})
}

func testedCode(testID, value string) node {
	return el("code", attrs(a("data-testid", testID)), text(value))
}

func passkeyRows(page ServerSettingsPage) node {
	return settingRows("Authentication — Passkeys (WebAuthn)",
		settingRow{"RDRS_WEBAUTHN_RP_ID", testedCode("webauthn-rp-id", page.WebAuthnRPID), codeDefault("localhost"), text("Relying Party ID for passkeys")},
		settingRow{"RDRS_WEBAUTHN_RP_ORIGIN", testedCode("webauthn-rp-origin", page.WebAuthnRPOrigin), codeDefault("http://localhost:{port}"), text("Relying Party origin URL (must match the URL you deploy at)")},
		settingRow{"RDRS_WEBAUTHN_RP_NAME", testedCode("webauthn-rp-name", page.WebAuthnRPName), codeDefault("rdrs"), text("Relying Party display name")})
}

func forwardAuthRows(page ServerSettingsPage) node {
	return settingRows("Authentication — Forward-Auth",
		settingRow{"RDRS_AUTH_PROXY_HEADER", currentText(page.AuthProxyHeader), unsetDefault(), text("Username header from the forward-auth proxy")},
		settingRow{"RDRS_TRUSTED_PROXY_NETWORKS", currentText(page.TrustedProxyNetworks), unsetDefault(), text("CIDRs the TCP peer must match")},
		settingRow{"RDRS_AUTH_PROXY_USER_CREATION", yesNo(page.AuthProxyUserCreation), codeDefault("false"), text("JIT-create unknown users")},
		settingRow{"RDRS_AUTH_PROXY_GROUPS_HEADER", currentText(page.AuthProxyGroupsHeader), unsetDefault(), text("Groups header from the forward-auth proxy")},
		settingRow{"RDRS_AUTH_PROXY_ADMIN_GROUP", currentText(page.AuthProxyAdminGroup), unsetDefault(), text("Group membership that grants admin")},
		settingRow{"RDRS_DISABLE_LOCAL_AUTH", yesNo(page.DisableLocalAuth), codeDefault("false"), text("Hide the browser password login form")},
		settingRow{"RDRS_AUTH_PROXY_LOGOUT_URL", currentText(page.AuthProxyLogoutURL), unsetDefault(), text("Sign Out redirects here to end the SSO session")})
}

func contentRows(page ServerSettingsPage) node {
	origin := "custom"
	if page.UserAgentIsDefault {
		origin = "default"
	}
	secret := el("span", attrs(class("success-text")), text("Configured"))
	if page.SecretGenerated {
		secret = el("span", attrs(class("muted")), text("Auto-generated"))
	}
	return settingRows("Content & Media",
		settingRow{"RDRS_USER_AGENT",
			group(el("code", nil, text(page.UserAgent)), text(" "),
				el("span", attrs(class("muted")), text("("), text(origin), text(")"))),
			codeDefault("RDRS/{version} (...)"), text("User agent for HTTP requests")},
		settingRow{"RDRS_SECRET", secret, el("em", nil, text("(auto-generated)")), secretDescription(page.SecretGenerated)})
}

func secretDescription(generated bool) node {
	description := text("Root key for session cookies, image proxy URLs, and encrypting saved Linkding/Kagi credentials.")
	if !generated {
		return description
	}
	return group(description, text(" "),
		el("strong", nil, text("Auto-generated keys change on every restart")),
		text(", so integration credentials are stored unencrypted rather than being locked away by a key that will not survive the next boot. Set this to a fixed value to have them encrypted at rest."))
}
