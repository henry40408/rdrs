package views

import "strconv"

// AdminUserRow is one row of the /admin user table.
type AdminUserRow struct {
	ID           int64
	Username     string
	Role         string
	Disabled     bool
	CreatedAt    string
	CreatedAtISO string
	IsSelf       bool
	// AwaitingPassword marks an account that never set a password.
	AwaitingPassword bool
	// InviteExpiresAt is empty when no invite link is outstanding.
	InviteExpiresAt string
}

// AdminPage is the /admin page.
type AdminPage struct {
	Layout           *Layout
	CSRFToken        string
	Users            []AdminUserRow
	InviteLink       string
	CanCreateAccount bool
	NeedsReauth      bool
}

// Admin renders the /admin page.
func Admin(page AdminPage) string {
	layout := page.Layout
	body := el("div", attrs(class("app-layout")),
		el("rdrs-sidebar", attrs(a("active", "admin"))),
		el("main", attrs(class("main-content")),
			flashMount(layout.FlashMessages),
			el("div", attrs(class("page-content")),
				el("h1", nil, text("Admin Panel")),
				show(page.NeedsReauth, adminReauthBanner(page.CSRFToken)),
				show(page.InviteLink != "", adminInviteCallout(page.InviteLink)),
				adminCreateAccountSection(page),
				adminUsersTable(page))))
	return string(appPage(layout, "Admin Panel", moduleScript(layout.GitVersion, "admin.js"), body))
}

func adminReauthBanner(csrfToken string) node {
	return el("form", attrs(a("method", "post"), a("action", "/admin/reauth"), a("class", "reauth-banner"),
		a("data-testid", "admin-reauth-form")),
		csrfField(csrfToken),
		text(" "),
		el("p", attrs(class("reauth-banner__body")), text("Confirm your password to change accounts. Creating, promoting, disabling, deleting and viewing as another user all need it.")),
		el("div", attrs(class("reauth-banner__row")),
			el("label", attrs(class("sr-only"), a("for", "admin-reauth-password")), text("Password")),
			text(" "),
			void("input", a("type", "password"), a("id", "admin-reauth-password"), a("name", "password"), flag("required"),
				a("autocomplete", "current-password"), a("placeholder", "Your password"), a("data-testid", "admin-reauth-password")),
			text(" "),
			el("button", attrs(a("type", "submit"), a("data-testid", "admin-reauth-submit")), text("Confirm"))))
}

// adminInviteCallout is not a flash: it holds the only copy of the link
// (stored as an HMAC) and must stay until copied.
func adminInviteCallout(link string) node {
	return el("section", attrs(class("invite-callout"), a("data-testid", "admin-invite-link")),
		el("h2", attrs(class("invite-callout__title")), text("One-time link")),
		el("p", attrs(class("invite-callout__body")), text("Send this to the person the account belongs to. It is shown "),
			el("strong", nil, text("once")), text(", works "), el("strong", nil, text("once")), text(", and expires in 7 days.")),
		el("div", attrs(class("invite-callout__row")),
			el("label", attrs(class("sr-only"), a("for", "invite-link")), text("One-time link")),
			text(" "),
			void("input", a("type", "text"), a("id", "invite-link"), a("class", "invite-callout__input"), a("value", link),
				flag("readonly"), a("data-testid", "admin-invite-link-value")),
			text(" "),
			el("button", attrs(a("type", "button"), a("data-copy-target", "#invite-link"), a("data-testid", "admin-invite-copy")),
				el("span", attrs(class("action-label")), text("Copy")))))
}

func adminCreateAccountSection(page AdminPage) node {
	if !page.CanCreateAccount {
		return el("section", attrs(class("admin-create-account")),
			el("h2", nil, text("Add an account")),
			el("p", attrs(class("muted"), a("data-testid", "admin-create-user-unavailable")),
				text("This instance is single-user. Set "), el("code", nil, text("RDRS_MULTI_USER_ENABLED=true")), text(" to add accounts.")))
	}
	return el("section", attrs(class("admin-create-account")),
		el("h2", nil, text("Add an account")),
		el("p", attrs(class("muted")), text("Creating an account issues a one-time link. Send it to the person it belongs to — they choose their own password. The link is shown once and expires in a week.")),
		adminCreateAccountForm(page.CSRFToken))
}

func adminCreateAccountForm(csrfToken string) node {
	return el("form", attrs(a("method", "post"), a("action", "/admin/users"), a("class", "admin-create-account__form"),
		a("data-testid", "admin-create-user-form")),
		csrfField(csrfToken),
		text(" "),
		el("div", attrs(class("form-group")),
			el("label", attrs(a("for", "new-username")), text("Username")),
			text(" "),
			void("input", a("type", "text"), a("id", "new-username"), a("name", "username"), flag("required"),
				a("autocomplete", "off"), a("data-testid", "admin-new-username"))),
		el("div", attrs(class("form-group")),
			el("label", attrs(a("for", "new-role")), text("Role")),
			text(" "),
			el("select", attrs(a("id", "new-role"), a("name", "role"), a("data-testid", "admin-new-role")),
				el("option", attrs(a("value", "user"), flag("selected")), text("user")),
				el("option", attrs(a("value", "admin")), text("admin")))),
		el("button", attrs(a("type", "submit"), a("data-testid", "admin-create-user-submit")), text("Create account")))
}

func adminUsersTable(page AdminPage) node {
	header := el("thead", nil, el("tr", nil,
		el("th", nil, text("ID")), el("th", nil, text("Username")), el("th", nil, text("Role")),
		el("th", nil, text("Status")), el("th", nil, text("Created")), el("th", nil, text("Actions"))))
	empty := show(len(page.Users) == 0, el("tr", nil,
		el("td", attrs(a("colspan", "6"), a("class", "empty-state-compact")), text("No users found."))))
	rows := each(page.Users, func(user AdminUserRow) node { return adminUserRow(user, page.CSRFToken) })
	return el("table", attrs(class("mobile-cards", "admin-users-table"), a("data-testid", "admin-users-table")),
		header, el("tbody", nil, rows, empty))
}

func adminUserRow(user AdminUserRow, csrfToken string) node {
	id := strconv.FormatInt(user.ID, 10)
	actions := adminRowActions(user, csrfToken)
	if user.IsSelf {
		actions = el("span", attrs(class("muted")), text("(you)"))
	}
	return el("tr", attrs(a("data-testid", "admin-user-row-"+id)),
		el("td", attrs(a("data-label", "ID")), text(id)),
		el("td", attrs(a("data-label", "Username")), text(user.Username)),
		el("td", attrs(a("data-label", "Role")), text(user.Role)),
		el("td", attrs(a("data-label", "Status")), adminStatusCell(user)),
		el("td", attrs(a("data-label", "Created")),
			el("time", attrs(a("datetime", user.CreatedAtISO), flag("data-local-text")), text(user.CreatedAt))),
		el("td", attrs(class("actions")), actions))
}

func adminStatusCell(user AdminUserRow) node {
	status := el("span", attrs(class("success-text")), text("active"))
	switch {
	case user.Disabled:
		status = el("span", attrs(class("error-text"), a("data-testid", "admin-user-disabled")), text("disabled"))
	case user.AwaitingPassword:
		status = el("span", attrs(class("muted"), a("data-testid", "admin-user-pending")), text("awaiting password"))
	}
	if user.InviteExpiresAt == "" {
		return status
	}
	return group(status, void("br"),
		el("span", attrs(class("muted", "admin-invite-note"), a("data-testid", "admin-user-invite")),
			text("link expires "), text(user.InviteExpiresAt)))
}

// adminActionForm is one inline POST button of a user row.
func adminActionForm(action, csrfToken string, extra node, formAttrs []attr, button node) node {
	attributes := append(attrs(a("method", "post"), a("action", action), a("class", "form-inline")), formAttrs...)
	return el("form", attributes, csrfField(csrfToken), text(" "), extra, button)
}

func adminRowActions(user AdminUserRow, csrfToken string) node {
	base := "/admin/users/" + strconv.FormatInt(user.ID, 10)
	isAdmin := user.Role == "admin"
	newRole, roleLabel := "admin", "promote"
	if isAdmin {
		newRole, roleLabel = "user", "demote"
	}
	disabledValue, statusLabel := "true", "disable"
	if user.Disabled {
		disabledValue, statusLabel = "false", "enable"
	}
	inviteLabel := "reset password"
	if user.AwaitingPassword {
		inviteLabel = "new link"
	}
	hidden := func(name, value string) node {
		return group(void("input", a("type", "hidden"), a("name", name), a("value", value)), text(" "))
	}
	submit := func(extra ...attr) []attr { return append(attrs(a("type", "submit"), class("action-link")), extra...) }
	const confirmInvite = "Issue a new link? Any existing one stops working. The account's current password keeps working until the new link is used."
	revoke := show(user.InviteExpiresAt != "", group(
		adminActionForm(base+"/invite/revoke", csrfToken, text(""), nil,
			el("button", submit(a("data-testid", "admin-invite-revoke-btn")), text("revoke link"))),
		text(" ")))
	return el("div", attrs(class("admin-actions")),
		adminActionForm(base+"/role", csrfToken, hidden("role", newRole), nil, el("button", submit(), text(roleLabel))),
		text(" "),
		adminActionForm(base+"/status", csrfToken, hidden("disabled", disabledValue), nil,
			el("button", submit(a("data-testid", "admin-disable-btn")), text(statusLabel))),
		text(" "),
		adminActionForm(base+"/invite", csrfToken, text(""), attrs(a("data-confirm", confirmInvite)),
			el("button", submit(a("data-testid", "admin-invite-btn")), text(inviteLabel))),
		text(" "),
		revoke,
		adminActionForm(base+"/masquerade", csrfToken, text(""), nil, el("button", submit(), text("view as"))),
		text(" "),
		adminActionForm(base+"/delete", csrfToken, text(""), attrs(a("data-confirm", "Delete user? This cannot be undone.")),
			el("button", attrs(a("type", "submit"), a("class", "action-link action-link-danger")), text("delete"))))
}
