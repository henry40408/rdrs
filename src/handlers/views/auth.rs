//! Logged-out account pages: login, first-run setup, invite redemption.

use maud::{Markup, html};

use super::layout::{DocumentParts, document, module_script};
use super::macros;
use crate::handlers::pages::{InviteTemplate, LoginTemplate, SetupTemplate};

pub(crate) fn login(page: &LoginTemplate) -> Markup {
    let body = html! {
        div.auth-layout {
            div.auth-card data-next=[page.next.as_deref()] {
                (macros::flash(&page.flash_messages))
                h1 { "rdrs" }
                // Filled server-side after a failed native POST, or by login.js on the fetch path.
                div id="error" class="error" hidden[page.error.is_none()] data-testid="login-error" {
                    @if let Some(error) = &page.error { (error) }
                }
                (passkey_section(page.local_auth_enabled))
                @if page.local_auth_enabled { (login_form(page)) }
                @if page.setup_available {
                    div.link {
                        a href="/setup" { "Set up this instance" }
                    }
                }
            }
        }
    };
    document(DocumentParts {
        title: "Login - RDRS",
        git_version: page.git_version,
        root: None,
        head: module_script(page.git_version, "login.js"),
        body,
    })
}

fn passkey_section(local_auth_enabled: bool) -> Markup {
    html! {
        div id="passkey-section" hidden {
            button type="button" id="passkey-login-btn" class="auth-passkey-btn" { " Login with Passkey " }
            @if local_auth_enabled {
                p.muted.auth-divider { "or use password" }
            }
        }
    }
}

fn login_form(page: &LoginTemplate) -> Markup {
    html! {
        // Real action/method so a no-JS submit never puts the password in a GET URL.
        form id="login-form" method="post" action="/login" data-testid="login-form" {
            (macros::csrf_field(&page.csrf_token))
            " "
            @if let Some(next) = &page.next {
                input type="hidden" name="next" value=(next);
                " "
            }
            div.form-group {
                label for="username" { "Username" }
                " "
                input type="text" id="username" name="username" required autocomplete="username" data-testid="username-input";
            }
            div.form-group {
                label for="password" { "Password" }
                " "
                input type="password" id="password" name="password" required autocomplete="current-password" data-testid="password-input";
            }
            button type="submit" class="btn-block" data-testid="login-submit" { "Sign In" }
        }
    }
}

/// Hint under a new-password field; shared by setup and invite.
fn password_hint(min_length: usize) -> Markup {
    html! {
        p.muted.form-hint {
            "At least " (min_length) " characters. A phrase of a few unrelated words works well — no mixture of cases or symbols is required."
        }
    }
}

pub(crate) fn setup(page: &SetupTemplate) -> Markup {
    let min = page.password_min_length;
    let max = page.password_max_length;
    let body = html! {
        div.auth-layout {
            div.auth-card {
                (macros::flash(&page.flash_messages))
                h1 { "rdrs" }
                p.muted.auth-lead { "Create the administrator account for this instance. Everyone else is added from the admin panel afterwards." }
                // Filled server-side after a failed native POST, or by setup.js on the fetch path.
                div id="error" class="error" hidden[page.error.is_none()] data-testid="setup-error" {
                    @if let Some(error) = &page.error { (error) }
                }
                form id="setup-form" method="post" action="/setup" data-testid="setup-form" {
                    (macros::csrf_field(&page.csrf_token))
                    " "
                    div.form-group {
                        label for="setup-token" { "Setup token" }
                        " "
                        input type="text" id="setup-token" name="setup-token" required autocomplete="off" spellcheck="false" data-testid="setup-token";
                        p.muted.form-hint { "Printed on the server's stderr when rdrs started with no accounts." }
                    }
                    div.form-group {
                        label for="username" { "Username" }
                        " "
                        input type="text" id="username" name="username" required autocomplete="username" data-testid="setup-username";
                    }
                    div.form-group {
                        label for="password" { "Password" }
                        " "
                        input type="password" id="password" name="password" required minlength=(min) maxlength=(max) autocomplete="new-password" data-testid="setup-password";
                        (password_hint(min))
                    }
                    div.form-group {
                        label for="confirm-password" { "Confirm Password" }
                        " "
                        input type="password" id="confirm-password" name="confirm-password" required minlength=(min) maxlength=(max) autocomplete="new-password" data-testid="setup-confirm-password";
                    }
                    button type="submit" class="btn-block" data-testid="setup-submit" { "Create Account" }
                }
                div.link {
                    a href="/login" { "Already have an account? Sign in" }
                }
            }
        }
    };
    document(DocumentParts {
        title: "Set up rdrs",
        git_version: page.git_version,
        root: None,
        head: module_script(page.git_version, "setup.js"),
        body,
    })
}

pub(crate) fn invite(page: &InviteTemplate) -> Markup {
    let body = html! {
        div.auth-layout {
            div.auth-card {
                h1 { "rdrs" }
                @if let Some(username) = &page.username {
                    (invite_form(page, username))
                } @else {
                    (invite_dead_end(page))
                }
                div.link {
                    a href="/login" { "Back to sign in" }
                }
            }
        }
    };
    document(DocumentParts {
        title: "Set your password - RDRS",
        git_version: page.git_version,
        root: None,
        head: html! {},
        body,
    })
}

fn invite_form(page: &InviteTemplate, username: &str) -> Markup {
    let min = page.password_min_length;
    let max = page.password_max_length;
    html! {
        @if let Some(error) = &page.error {
            div.error data-testid="invite-error" { (error) }
        }
        p.muted.auth-lead { "Choose a password for " strong { (username) } "." }
        // Plain form POST; the CSRF token comes from the anonymous session minted on GET.
        form method="post" action={ "/invite/" (page.token) } data-testid="invite-form" {
            (macros::csrf_field(&page.csrf_token))
            " "
            div.form-group {
                label for="password" { "Password" }
                " "
                input type="password" id="password" name="password" required minlength=(min) maxlength=(max) autocomplete="new-password" data-testid="invite-password";
                (password_hint(min))
            }
            div.form-group {
                label for="confirm-password" { "Confirm Password" }
                " "
                input type="password" id="confirm-password" name="confirm_password" required minlength=(min) maxlength=(max) autocomplete="new-password" data-testid="invite-confirm-password";
            }
            button type="submit" class="btn-block" data-testid="invite-submit" { "Set Password" }
        }
    }
}

/// One dead end for every failing token, naming no account or reason, so this
/// page cannot become an account oracle.
fn invite_dead_end(page: &InviteTemplate) -> Markup {
    html! {
        @if let Some(error) = &page.error {
            div.error data-testid="invite-error" { (error) }
        } @else {
            div.error data-testid="invite-invalid" { "This link is not valid. It may have expired or already been used." }
            p.muted { "Ask whoever set up your account for a new one." }
        }
    }
}
