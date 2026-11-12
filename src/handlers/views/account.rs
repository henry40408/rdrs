//! `/user-settings`.

use maud::{Markup, html};

use super::{layout, macros};
use crate::handlers::pages::{ApiTokenRow, SessionRow, UserSettingsTemplate};

pub(crate) fn page(t: &UserSettingsTemplate) -> Markup {
    let page = html! {
        div class="app-layout" {
            rdrs-sidebar active="user-settings" {}
            main class="main-content" {
                (macros::flash_mount(&t.layout.flash_messages))
                div class="page-content" {
                    h1 { "Settings" }
                    (account_information(t))
                    @if t.show_sessions {
                        (active_sessions(t))
                        (greader_api_tokens(t))
                    }
                    hr;
                    (rss_client(t))
                    hr;
                    (change_password(t))
                    hr;
                    h2 { "Passkeys" }
                    p class="muted" { "Passkeys let you sign in without a password using your device's biometrics or security key." }
                    rdrs-passkeys {}
                    hr;
                    (display_preferences(t))
                    hr;
                    (integrations(t))
                }
            }
        }
    };
    let script = layout::module_script(t.layout.git_version, "passkey.js");
    layout::app_page(&t.layout, t.title, Some(script), page)
}

fn local_time(iso: &str, text: &str) -> Markup {
    html! { time datetime=(iso) data-local-text { (text) } }
}

fn account_information(t: &UserSettingsTemplate) -> Markup {
    html! {
        h2 { "Account Information" }
        table {
            tr { th class="settings-th" { "Username" } td { (t.username) } }
            tr { th class="settings-th" { "Role" } td { (t.role) } }
            tr { th class="settings-th" { "Registered" } td { (local_time(&t.created_at_iso, &t.created_at)) } }
            tr { th class="settings-th" { "Logged In" } td { (local_time(&t.session_created_at_iso, &t.session_created_at)) } }
        }
    }
}

/// What a session and a `GReader` token card both show.
struct CredentialMeta<'a> {
    user_agent: &'a str,
    ip_address: &'a str,
    created_at_iso: &'a str,
    created_at: &'a str,
    last_seen_iso: &'a str,
    last_seen: &'a str,
    expires_at_iso: &'a str,
    expires_at: &'a str,
}

impl<'a> From<&'a SessionRow> for CredentialMeta<'a> {
    fn from(s: &'a SessionRow) -> Self {
        Self {
            user_agent: &s.user_agent,
            ip_address: &s.ip_address,
            created_at_iso: &s.created_at_iso,
            created_at: &s.created_at,
            last_seen_iso: &s.last_seen_iso,
            last_seen: &s.last_seen,
            expires_at_iso: &s.expires_at_iso,
            expires_at: &s.expires_at,
        }
    }
}

impl<'a> From<&'a ApiTokenRow> for CredentialMeta<'a> {
    fn from(t: &'a ApiTokenRow) -> Self {
        Self {
            user_agent: &t.user_agent,
            ip_address: &t.ip_address,
            created_at_iso: &t.created_at_iso,
            created_at: &t.created_at,
            last_seen_iso: &t.last_seen_iso,
            last_seen: &t.last_seen,
            expires_at_iso: &t.expires_at_iso,
            expires_at: &t.expires_at,
        }
    }
}

fn credential_meta(meta: &CredentialMeta<'_>) -> Markup {
    html! {
        dl class="cred-card-meta" {
            div class="cred-card-meta-wide" { dt { "User agent" } dd class="cred-card-ua" { (meta.user_agent) } }
            div { dt { "IP" } dd { (meta.ip_address) } }
            div { dt { "Signed in" } dd { (local_time(meta.created_at_iso, meta.created_at)) } }
            div { dt { "Last active" } dd { (local_time(meta.last_seen_iso, meta.last_seen)) } }
            div { dt { "Expires" } dd { (local_time(meta.expires_at_iso, meta.expires_at)) } }
        }
    }
}

fn revoke_form(action: &str, csrf_token: &str) -> Markup {
    html! {
        form method="post" action=(action) {
            (macros::csrf_field(csrf_token))
            " "
            button type="submit" class="btn btn-secondary" { "Revoke" }
        }
    }
}

fn session_card(s: &SessionRow, csrf_token: &str) -> Markup {
    html! {
        li class={ "cred-card" @if s.is_current { " cred-card--current" } } {
            (credential_meta(&CredentialMeta::from(s)))
            div class="cred-card-actions" {
                @if s.is_current {
                    span class="cred-card-current" { "This device" }
                } @else {
                    (revoke_form(&format!("/user-settings/sessions/{}/revoke", s.id), csrf_token))
                }
            }
        }
    }
}

fn active_sessions(t: &UserSettingsTemplate) -> Markup {
    html! {
        hr;
        h2 { "Active Sessions" }
        ul class="cred-list" data-testid="sessions-list" {
            @for s in &t.sessions {
                (session_card(s, &t.csrf_token))
            }
        }
        form method="post" action="/user-settings/sessions/revoke-others" {
            (macros::csrf_field(&t.csrf_token))
            " "
            button type="submit" { "Sign out other sessions" }
        }
    }
}

fn api_token_card(token: &ApiTokenRow, csrf_token: &str) -> Markup {
    html! {
        li class="cred-card" {
            (credential_meta(&CredentialMeta::from(token)))
            div class="cred-card-actions" {
                (revoke_form(&format!("/user-settings/api-tokens/{}/revoke", token.id), csrf_token))
            }
        }
    }
}

fn greader_api_tokens(t: &UserSettingsTemplate) -> Markup {
    html! {
        hr;
        h2 { "GReader API Tokens" }
        p class="muted" {
            "These are Google Reader client tokens — issued to RSS apps (FeedMe, Read You, etc.) when they sign in below, kept separate from your browser sessions above. Revoking one forces that client to sign in again."
        }
        @if t.api_tokens.is_empty() {
            p class="muted" { "No GReader clients are currently signed in." }
        } @else {
            // `label` is omitted: ClientLogin sets it to the User-Agent, shown already.
            ul class="cred-list" data-testid="api-tokens-list" {
                @for token in &t.api_tokens {
                    (api_token_card(token, &t.csrf_token))
                }
            }
            form method="post" action="/user-settings/api-tokens/revoke-all" data-confirm="Revoke all API tokens? Every connected GReader client will need to sign in again." {
                (macros::csrf_field(&t.csrf_token))
                " "
                button type="submit" class="btn btn-secondary" { "Revoke all API tokens" }
            }
        }
    }
}

fn rss_client(t: &UserSettingsTemplate) -> Markup {
    html! {
        h2 { "RSS Client (Google Reader API)" }
        p class="muted" { "Connect any RSS reader that supports the Google Reader API (e.g., Reeder, NetNewsWire, FeedMe, Read You, FreshRSS)." }
        table {
            tr { th class="settings-th" { "Server URL" } td { code { (t.public_base_url) } } }
            tr { th class="settings-th" { "Username" } td { code { (t.username) } } }
            tr { th class="settings-th" { "Password" } td class="muted" { "Your RDRS password" } }
        }
        p class="muted" {
            "In your RSS client, choose \"Google Reader\" or \"FreshRSS\" as the account type and enter the server URL above with your credentials. Some clients may require the FreshRSS-compatible URL: "
            code { (t.public_base_url) "/api/greader.php" }
        }
    }
}

fn change_password(t: &UserSettingsTemplate) -> Markup {
    html! {
        h2 { "Change Password" }
        form method="post" action="/user-settings/password" {
            (macros::csrf_field(&t.csrf_token))
            " "
            div class="form-group" {
                label for="current-password" { "Current Password" }
                " "
                input type="password" id="current-password" name="current_password" required autocomplete="current-password" data-testid="current-password";
            }
            div class="form-group" {
                label for="new-password" { "New Password" }
                " "
                input type="password" id="new-password" name="new_password" required minlength=(t.password_min_length) maxlength=(t.password_max_length) autocomplete="new-password" data-testid="new-password";
                p class="muted form-hint" { "At least " (t.password_min_length) " characters. A phrase of a few unrelated words works well — no mixture of cases or symbols is required." }
            }
            div class="form-group" {
                label for="confirm-password" { "Confirm New Password" }
                " "
                input type="password" id="confirm-password" name="confirm_password" required minlength=(t.password_min_length) maxlength=(t.password_max_length) autocomplete="new-password" data-testid="confirm-new-password";
            }
            button type="submit" { "Change Password" }
        }
    }
}

fn suggestion_list<T: std::fmt::Display>(id: &str, values: &[T]) -> Markup {
    html! {
        datalist id=(id) {
            @for v in values {
                option value=(v) {}
            }
        }
    }
}

fn display_preferences(t: &UserSettingsTemplate) -> Markup {
    html! {
        h2 { "Display Preferences" }
        form method="post" action="/user-settings/preferences" {
            (macros::csrf_field(&t.csrf_token))
            " "
            (timezone_field(t))
            (theme_field(t))
            (entries_per_page_field(t))
            (retention_field(t))
            (sidebar_fields(t))
            (offline_keep_field(t))
            (pixel_tracking_field(t))
            button type="submit" { "Save Preferences" }
        }
    }
}

fn timezone_field(t: &UserSettingsTemplate) -> Markup {
    html! {
        div class="form-group" {
            label for="timezone" { "Timezone" }
            " "
            input type="text" id="timezone" name="timezone" value=(t.timezone) list="timezone-list" autocomplete="off" spellcheck="false" data-testid="timezone" required;
            (suggestion_list("timezone-list", t.timezone_suggestions))
            @if t.timezone_is_suggestion {
                span class="banner banner--warning" role="status" data-testid="timezone-suggestion" { "Filled in from your browser's timezone. It is not saved yet: check it, then press Save." }
            }
            span class="muted text-xs" {
                "An IANA name such as Asia/Taipei. Decides where "
                a href="/statistics" { "Statistics" }
                " cuts one day from the next; times elsewhere already show in your browser's timezone. Default UTC."
            }
        }
    }
}

fn theme_field(t: &UserSettingsTemplate) -> Markup {
    let theme = t.theme.as_deref();
    html! {
        div class="form-group" {
            label for="theme-select" { "Theme" }
            " "
            select id="theme-select" name="theme" data-testid="theme-select" {
                option value="system" selected[theme.is_none()] { "System (auto)" }
                option value="light" selected[theme == Some("light")] { "Light" }
                option value="dark" selected[theme == Some("dark")] { "Dark" }
            }
        }
    }
}

fn entries_per_page_field(t: &UserSettingsTemplate) -> Markup {
    html! {
        div class="form-group" {
            label for="entries-per-page" { "Entries per page" }
            " "
            input type="number" id="entries-per-page" name="entries_per_page" value=(t.entries_per_page) min="10" max="100" list="entries-per-page-list" autocomplete="off" data-testid="entries-per-page" required;
            (suggestion_list("entries-per-page-list", t.entries_per_page_suggestions))
            span class="muted text-xs" { "(10-100)" }
        }
    }
}

fn retention_field(t: &UserSettingsTemplate) -> Markup {
    html! {
        div class="form-group" {
            label for="retention-read-days" { "Delete read articles older than (days)" }
            " "
            input type="number" id="retention-read-days" name="retention_read_days" value=(t.retention_read_days) min="0" max="3650" list="retention-read-days-list" autocomplete="off" data-testid="retention-read-days" required;
            (suggestion_list("retention-read-days-list", t.retention_read_days_suggestions))
            span class="muted text-xs" { "(0 = never delete, max 3650)" }
        }
    }
}

fn sidebar_fields(t: &UserSettingsTemplate) -> Markup {
    html! {
        div class="form-group" {
            label for="sidebar-sort" { "Sidebar order" }
            " "
            select id="sidebar-sort" name="sidebar_sort" data-testid="sidebar-sort" {
                option value="name" selected[t.sidebar_sort == "name"] { "Name (A-Z)" }
                option value="unread" selected[t.sidebar_sort == "unread"] { "Unread count (descending)" }
            }
            " "
            span class="muted text-xs" { "Applies to categories and to the open category's feeds." }
        }
        div class="form-group" {
            label {
                " "
                input type="checkbox" id="sidebar-hide-read" name="sidebar_hide_read" value="1" checked[t.sidebar_hide_read] data-testid="sidebar-hide-read";
                " Hide fully-read categories and feeds "
            }
            " "
            span class="muted text-xs" { "The category or feed you are currently reading stays listed." }
        }
    }
}

fn offline_keep_field(t: &UserSettingsTemplate) -> Markup {
    html! {
        div class="form-group" {
            label for="offline-keep" { "Keep offline (entries)" }
            " "
            input type="number" id="offline-keep" name="offline_keep" value=(t.offline_keep) min="0" max="200" list="offline-keep-list" autocomplete="off" data-testid="offline-keep" required;
            (suggestion_list("offline-keep-list", t.offline_keep_suggestions))
            // The disk trade-off is stated where the number is chosen.
            span class="muted text-xs" {
                "(0 = off, max 200) Stores these articles in this browser so you can read them without a connection. Newest unread first, then starred. "
                a href="/entries/offline" data-testid="offline-library-link" { "See what is saved" }
                "."
            }
        }
    }
}

fn pixel_tracking_field(t: &UserSettingsTemplate) -> Markup {
    html! {
        div class="form-group" {
            label {
                " "
                input type="checkbox" id="pixel-tracking" name="pixel_tracking" value="1" checked[t.pixel_tracking_enabled] data-testid="pixel-tracking";
                " Track which entries get opened "
            }
            " "
            // Named outright: the reader is consenting to a tracking pixel.
            span class="muted text-xs" {
                "Adds an invisible 1\u{d7}1 image to each entry's content, so opening one is recorded even when it happens in another reader app. Only this server sees the request \u{2014} nothing leaves this instance. Shows up as an open rate on "
                a href="/feeds" { "Feeds" }
                " and "
                a href="/statistics" { "Statistics" }
                ", which is a better guide to a low-value feed than the read count. "
                strong { "Counts only entries that arrive after you switch this on" }
                ", so the rate starts out empty and fills in as your feeds update \u{2014} articles already in your library carry no image and are left out of the total."
            }
        }
    }
}

fn integrations(t: &UserSettingsTemplate) -> Markup {
    html! {
        h2 { "Integrations" }
        p class="muted" { "Connect external services to save articles." }
        // Undecryptable credentials read as "not configured"; warn before an
        // overwrite that a restored RDRS_SECRET would have made unnecessary.
        @if t.credentials_unreadable {
            (credentials_unreadable_banner())
        }
        (linkding(t))
        (kagi(t))
    }
}

fn credentials_unreadable_banner() -> Markup {
    html! {
        div class="banner banner--warning" role="alert" data-testid="credentials-unreadable" {
            span class="banner-icon" aria-hidden="true" {}
            div class="banner-body" {
                span class="sr-only" { "Warning: " }
                " "
                span class="banner-message" { "Your saved integration credentials cannot be decrypted with the current " code { "RDRS_SECRET" } ". They are still stored. Restore the previous secret to recover them, or re-enter them below to replace them." }
            }
        }
    }
}

fn clear_form(action: &str, confirm: &str, button_label: &str, csrf_token: &str) -> Markup {
    html! {
        form method="post" action=(action) class="form-inline-block" data-confirm=(confirm) {
            (macros::csrf_field(csrf_token))
            " "
            input type="hidden" name="_clear" value="1";
            " "
            button type="submit" class="btn btn-secondary" { (button_label) }
        }
    }
}

fn linkding(t: &UserSettingsTemplate) -> Markup {
    html! {
        h3 { "Linkding" }
        p class="muted" {
            a href="https://github.com/sissbruecker/linkding" target="_blank" rel="noopener noreferrer" { "Linkding" }
            " is a self-hosted bookmark manager. "
            @if t.linkding_configured { span class="success-text" { "Configured" } }
        }
        form method="post" action="/user-settings/linkding" {
            (macros::csrf_field(&t.csrf_token))
            " "
            div class="form-group" {
                label for="linkding-api-url" { "API URL" }
                " "
                input type="url" id="linkding-api-url" name="api_url" value=(t.linkding_api_url) placeholder="https://linkding.example.com";
            }
            div class="form-group" {
                label for="linkding-api-token" { "API Token" }
                " "
                input type="password" id="linkding-api-token" name="api_token" placeholder=(if t.linkding_configured { "(unchanged)" } else { "Enter your API token" });
            }
            button type="submit" { "Save Linkding Settings" }
        }
        @if t.linkding_configured {
            (clear_form("/user-settings/linkding", "Clear Linkding settings?", "Clear Linkding", &t.csrf_token))
        }
    }
}

fn language_option(code: &str, label: &str, current: Option<&str>) -> Markup {
    html! { option value=(code) selected[current == Some(code)] { (label) } }
}

fn kagi(t: &UserSettingsTemplate) -> Markup {
    let language = t.kagi_language.as_deref();
    html! {
        h3 { "Kagi Universal Summarizer" }
        p class="muted" {
            a href="https://kagi.com/summarizer" target="_blank" rel="noopener noreferrer" { "Kagi Universal Summarizer" }
            " provides AI-powered article summaries. "
            @if t.kagi_configured { span class="success-text" { "Configured" } }
        }
        form method="post" action="/user-settings/kagi" {
            (macros::csrf_field(&t.csrf_token))
            " "
            div class="form-group" {
                label for="kagi-session-link" { "Session Link" }
                " "
                input type="text" id="kagi-session-link" name="session_link" placeholder=(if t.kagi_configured { "(unchanged)" } else { "Paste your session link" });
            }
            div class="form-group" {
                label for="kagi-language" { "Target Language" }
                " "
                select id="kagi-language" name="language" {
                    option value="" selected[language.is_none()] { "Auto-detect" }
                    (language_option("EN", "English", language))
                    (language_option("ZH-HANT", "繁體中文", language))
                    (language_option("ZH-CN", "简体中文", language))
                    (language_option("JA", "日本語", language))
                    (language_option("KO", "한국어", language))
                    (language_option("DE", "Deutsch", language))
                    (language_option("FR", "Français", language))
                    (language_option("ES", "Español", language))
                    (language_option("PT", "Português", language))
                }
            }
            button type="submit" { "Save Kagi Settings" }
        }
        @if t.kagi_configured {
            (clear_form("/user-settings/kagi", "Clear Kagi settings?", "Clear Kagi", &t.csrf_token))
        }
    }
}
