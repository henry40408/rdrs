//! `/settings` server-config table.

use maud::{Markup, html};

use super::{layout, macros};
use crate::handlers::pages::SettingsTemplate;

pub(crate) fn page(t: &SettingsTemplate) -> Markup {
    let page = html! {
        div class="app-layout" {
            rdrs-sidebar active="settings" {}
            main class="main-content" {
                (macros::flash_mount(&t.layout.flash_messages))
                div class="page-content" {
                    h1 { "App" }
                    div id="settings-body" {
                        p class="muted" { "Version: " code { (t.git_version) } }

                        h2 { "Configuration" }
                        p class="muted" { "These settings are configured via environment variables and cannot be changed at runtime. The current column reflects the running instance." }
                        table class="mobile-cards-settings" {
                            thead {
                                tr {
                                    th class="settings-th" { "Variable" }
                                    th class="settings-th settings-col-current" { "Current" }
                                    th class="settings-th" { "Default" }
                                    th class="settings-th" { "Description" }
                                }
                            }
                            tbody {
                                (server_rows(t))
                                (account_rows(t))
                                (passkey_rows(t))
                                (forward_auth_rows(t))
                                (content_rows(t))
                            }
                        }
                    }
                }
            }
        }
    };
    layout::app_page(&t.layout, t.title, None, page)
}

fn group_header(name: &str) -> Markup {
    html! { tr class="grouphdr" { td colspan="4" { (name) } } }
}

fn yes_no(value: bool) -> Markup {
    html! {
        @if value {
            span class="success-text" { "Yes" }
        } @else {
            span class="muted" { "No" }
        }
    }
}

fn unset_default() -> Markup {
    html! { em { "(unset)" } }
}

fn code_default(value: &str) -> Markup {
    html! { code { (value) } }
}

struct SettingRow<'a> {
    variable: &'a str,
    current: Markup,
    default: Markup,
    description: Markup,
}

fn setting_row(row: SettingRow<'_>) -> Markup {
    html! {
        tr {
            td { code { (row.variable) } }
            td class="settings-col-current" data-label="Current" { (row.current) }
            td data-label="Default" { (row.default) }
            td data-label="Description" { (row.description) }
        }
    }
}

fn server_rows(t: &SettingsTemplate) -> Markup {
    html! {
        (group_header("Server"))
        (setting_row(SettingRow {
            variable: "DATABASE_URL",
            current: html! { code { (t.database_url) } },
            default: code_default("rdrs.sqlite3"),
            description: html! { "SQLite database file path" },
        }))
        (setting_row(SettingRow {
            variable: "RDRS_SERVER_BIND",
            current: html! { code { (t.server_bind) } },
            default: code_default("127.0.0.1:8080"),
            description: html! { "HTTP server bind address (host:port)" },
        }))
    }
}

fn account_rows(t: &SettingsTemplate) -> Markup {
    html! {
        (group_header("Accounts"))
        (setting_row(SettingRow {
            variable: "RDRS_MULTI_USER_ENABLED",
            current: yes_no(t.multi_user_enabled),
            default: code_default("false"),
            description: html! { "Allow multiple users" },
        }))
    }
}

fn passkey_rows(t: &SettingsTemplate) -> Markup {
    html! {
        (group_header("Authentication \u{2014} Passkeys (WebAuthn)"))
        (setting_row(SettingRow {
            variable: "RDRS_WEBAUTHN_RP_ID",
            current: html! { code data-testid="webauthn-rp-id" { (t.webauthn_rp_id) } },
            default: code_default("localhost"),
            description: html! { "Relying Party ID for passkeys" },
        }))
        (setting_row(SettingRow {
            variable: "RDRS_WEBAUTHN_RP_ORIGIN",
            current: html! { code data-testid="webauthn-rp-origin" { (t.webauthn_rp_origin) } },
            default: code_default("http://localhost:{port}"),
            description: html! { "Relying Party origin URL (must match the URL you deploy at)" },
        }))
        (setting_row(SettingRow {
            variable: "RDRS_WEBAUTHN_RP_NAME",
            current: html! { code data-testid="webauthn-rp-name" { (t.webauthn_rp_name) } },
            default: code_default("rdrs"),
            description: html! { "Relying Party display name" },
        }))
    }
}

fn forward_auth_rows(t: &SettingsTemplate) -> Markup {
    html! {
        (group_header("Authentication \u{2014} Forward-Auth"))
        (setting_row(SettingRow {
            variable: "RDRS_AUTH_PROXY_HEADER",
            current: macros::current_text(&t.auth_proxy_header),
            default: unset_default(),
            description: html! { "Username header from the forward-auth proxy" },
        }))
        (setting_row(SettingRow {
            variable: "RDRS_TRUSTED_PROXY_NETWORKS",
            current: macros::current_text(&t.trusted_proxy_networks),
            default: unset_default(),
            description: html! { "CIDRs the TCP peer must match" },
        }))
        (setting_row(SettingRow {
            variable: "RDRS_AUTH_PROXY_USER_CREATION",
            current: yes_no(t.auth_proxy_user_creation),
            default: code_default("false"),
            description: html! { "JIT-create unknown users" },
        }))
        (setting_row(SettingRow {
            variable: "RDRS_AUTH_PROXY_GROUPS_HEADER",
            current: macros::current_text(&t.auth_proxy_groups_header),
            default: unset_default(),
            description: html! { "Groups header from the forward-auth proxy" },
        }))
        (setting_row(SettingRow {
            variable: "RDRS_AUTH_PROXY_ADMIN_GROUP",
            current: macros::current_text(&t.auth_proxy_admin_group),
            default: unset_default(),
            description: html! { "Group membership that grants admin" },
        }))
        (setting_row(SettingRow {
            variable: "RDRS_DISABLE_LOCAL_AUTH",
            current: yes_no(t.disable_local_auth),
            default: code_default("false"),
            description: html! { "Hide the browser password login form" },
        }))
        (setting_row(SettingRow {
            variable: "RDRS_AUTH_PROXY_LOGOUT_URL",
            current: macros::current_text(&t.auth_proxy_logout_url),
            default: unset_default(),
            description: html! { "Sign Out redirects here to end the SSO session" },
        }))
    }
}

fn content_rows(t: &SettingsTemplate) -> Markup {
    html! {
        (group_header("Content & Media"))
        (setting_row(SettingRow {
            variable: "RDRS_USER_AGENT",
            current: html! {
                code { (t.user_agent) }
                " "
                span class="muted" {
                    "("
                    @if t.user_agent_is_default { "default" } @else { "custom" }
                    ")"
                }
            },
            default: code_default("RDRS/{version} (...)"),
            description: html! { "User agent for HTTP requests" },
        }))
        (setting_row(SettingRow {
            variable: "RDRS_SECRET",
            current: html! {
                @if t.secret_generated {
                    span class="muted" { "Auto-generated" }
                } @else {
                    span class="success-text" { "Configured" }
                }
            },
            default: html! { em { "(auto-generated)" } },
            description: secret_description(t.secret_generated),
        }))
    }
}

fn secret_description(secret_generated: bool) -> Markup {
    html! {
        "Root key for session cookies, image proxy URLs, and encrypting saved Linkding/Kagi credentials."
        @if secret_generated {
            " "
            strong { "Auto-generated keys change on every restart" }
            ", so integration credentials are stored unencrypted rather than being locked away by a key that will not survive the next boot. Set this to a fixed value to have them encrypted at rest."
        }
    }
}
