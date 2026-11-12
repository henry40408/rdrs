//! `/admin`.

use maud::{Markup, html};

use super::layout::{app_page, module_script};
use super::macros;
use crate::handlers::pages::{AdminTemplate, AdminUserView};

pub(crate) fn admin(page: &AdminTemplate) -> Markup {
    let layout = &page.layout;
    let body = html! {
        div.app-layout {
            rdrs-sidebar active="admin" {}
            main.main-content {
                (macros::flash_mount(&layout.flash_messages))
                div.page-content {
                    h1 { "Admin Panel" }
                    @if page.needs_reauth { (reauth_banner(&page.csrf_token)) }
                    @if let Some(link) = &page.invite_link { (invite_callout(link)) }
                    (create_account_section(page))
                    (users_table(page))
                }
            }
        }
    };
    let script = module_script(layout.git_version, "admin.js");
    app_page(layout, page.title, Some(script), body)
}

fn reauth_banner(csrf_token: &str) -> Markup {
    html! {
        form method="post" action="/admin/reauth" class="reauth-banner" data-testid="admin-reauth-form" {
            (macros::csrf_field(csrf_token))
            " "
            p.reauth-banner__body { "Confirm your password to change accounts. Creating, promoting, disabling, deleting and viewing as another user all need it." }
            div.reauth-banner__row {
                label.sr-only for="admin-reauth-password" { "Password" }
                " "
                input type="password" id="admin-reauth-password" name="password" required autocomplete="current-password" placeholder="Your password" data-testid="admin-reauth-password";
                " "
                button type="submit" data-testid="admin-reauth-submit" { "Confirm" }
            }
        }
    }
}

/// Not a flash: this is the only copy of the link (stored as an HMAC) and must
/// stay until copied.
fn invite_callout(link: &str) -> Markup {
    html! {
        section.invite-callout data-testid="admin-invite-link" {
            h2.invite-callout__title { "One-time link" }
            p.invite-callout__body { "Send this to the person the account belongs to. It is shown " strong { "once" } ", works " strong { "once" } ", and expires in 7 days." }
            div.invite-callout__row {
                label.sr-only for="invite-link" { "One-time link" }
                " "
                input type="text" id="invite-link" class="invite-callout__input" value=(link) readonly data-testid="admin-invite-link-value";
                " "
                button type="button" data-copy-target="#invite-link" data-testid="admin-invite-copy" { span.action-label { "Copy" } }
            }
        }
    }
}

fn create_account_section(page: &AdminTemplate) -> Markup {
    html! {
        section.admin-create-account {
            h2 { "Add an account" }
            @if page.can_create_account {
                // No public sign-up: the user sets their password via a one-time link.
                p.muted { "Creating an account issues a one-time link. Send it to the person it belongs to — they choose their own password. The link is shown once and expires in a week." }
                (create_account_form(&page.csrf_token))
            } @else {
                // Explain single-user mode rather than silently omit the button.
                p.muted data-testid="admin-create-user-unavailable" { "This instance is single-user. Set " code { "RDRS_MULTI_USER_ENABLED=true" } " to add accounts." }
            }
        }
    }
}

fn create_account_form(csrf_token: &str) -> Markup {
    html! {
        form method="post" action="/admin/users" class="admin-create-account__form" data-testid="admin-create-user-form" {
            (macros::csrf_field(csrf_token))
            " "
            div.form-group {
                label for="new-username" { "Username" }
                " "
                input type="text" id="new-username" name="username" required autocomplete="off" data-testid="admin-new-username";
            }
            div.form-group {
                label for="new-role" { "Role" }
                " "
                select id="new-role" name="role" data-testid="admin-new-role" {
                    option value="user" selected { "user" }
                    option value="admin" { "admin" }
                }
            }
            button type="submit" data-testid="admin-create-user-submit" { "Create account" }
        }
    }
}

fn users_table(page: &AdminTemplate) -> Markup {
    html! {
        table.mobile-cards.admin-users-table data-testid="admin-users-table" {
            thead {
                tr {
                    th { "ID" } th { "Username" } th { "Role" } th { "Status" } th { "Created" } th { "Actions" }
                }
            }
            tbody {
                @for user in &page.users { (user_row(user, &page.csrf_token)) }
                @if page.users.is_empty() {
                    tr { td colspan="6" class="empty-state-compact" { "No users found." } }
                }
            }
        }
    }
}

fn user_row(user: &AdminUserView, csrf_token: &str) -> Markup {
    html! {
        tr data-testid={ "admin-user-row-" (user.id) } {
            td data-label="ID" { (user.id) }
            td data-label="Username" { (user.username) }
            td data-label="Role" { (user.role) }
            td data-label="Status" { (status_cell(user)) }
            td data-label="Created" { time datetime=(user.created_at_iso) data-local-text { (user.created_at) } }
            td.actions {
                @if user.is_self {
                    span.muted { "(you)" }
                } @else {
                    (row_actions(user, csrf_token))
                }
            }
        }
    }
}

fn status_cell(user: &AdminUserView) -> Markup {
    html! {
        @if user.disabled {
            span.error-text data-testid="admin-user-disabled" { "disabled" }
        } @else if user.awaiting_password {
            span.muted data-testid="admin-user-pending" { "awaiting password" }
        } @else {
            span.success-text { "active" }
        }
        @if let Some(expires) = &user.invite_expires_at {
            br;
            span.muted.admin-invite-note data-testid="admin-user-invite" { "link expires " (expires) }
        }
    }
}

fn row_actions(user: &AdminUserView, csrf_token: &str) -> Markup {
    let id = user.id;
    let is_admin = user.role == "admin";
    html! {
        div.admin-actions {
            form method="post" action={ "/admin/users/" (id) "/role" } class="form-inline" {
                (macros::csrf_field(csrf_token))
                " "
                input type="hidden" name="role" value=(if is_admin { "user" } else { "admin" });
                " "
                button type="submit" class="action-link" { (if is_admin { "demote" } else { "promote" }) }
            }
            " "
            form method="post" action={ "/admin/users/" (id) "/status" } class="form-inline" {
                (macros::csrf_field(csrf_token))
                " "
                input type="hidden" name="disabled" value=(if user.disabled { "false" } else { "true" });
                " "
                button type="submit" class="action-link" data-testid="admin-disable-btn" { (if user.disabled { "enable" } else { "disable" }) }
            }
            " "
            form method="post" action={ "/admin/users/" (id) "/invite" } class="form-inline" data-confirm="Issue a new link? Any existing one stops working. The account's current password keeps working until the new link is used." {
                (macros::csrf_field(csrf_token))
                " "
                button type="submit" class="action-link" data-testid="admin-invite-btn" { (if user.awaiting_password { "new link" } else { "reset password" }) }
            }
            " "
            @if user.invite_expires_at.is_some() {
                form method="post" action={ "/admin/users/" (id) "/invite/revoke" } class="form-inline" {
                    (macros::csrf_field(csrf_token))
                    " "
                    button type="submit" class="action-link" data-testid="admin-invite-revoke-btn" { "revoke link" }
                }
                " "
            }
            form method="post" action={ "/admin/users/" (id) "/masquerade" } class="form-inline" {
                (macros::csrf_field(csrf_token))
                " "
                button type="submit" class="action-link" { "view as" }
            }
            " "
            form method="post" action={ "/admin/users/" (id) "/delete" } class="form-inline" data-confirm="Delete user? This cannot be undone." {
                (macros::csrf_field(csrf_token))
                " "
                button type="submit" class="action-link action-link-danger" { "delete" }
            }
        }
    }
}
