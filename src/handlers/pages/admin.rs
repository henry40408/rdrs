use askama::Template;
use axum::extract::State;

use super::{AppLayoutContext, build_app_layout};
use crate::AppState;
use crate::middleware::auth::{PageAdminUser, PageAuthUser};
use crate::middleware::flash::Flash;

/// Pull a one-time invite link out of the flash. The message is replaced, not
/// dropped: `Flash` only clears the cookie when messages remain, so an empty
/// list would leave the link to reappear on the next load.
fn extract_invite_link(flash: &mut Flash) -> Option<String> {
    let link = flash
        .messages
        .iter()
        .find(|m| m.message.contains("/invite/"))
        .map(|m| m.message.clone())?;

    flash.messages = vec![crate::middleware::flash::FlashMessage::success(
        "Account link ready.",
    )];
    Some(link)
}

pub async fn admin_page(
    admin: PageAdminUser,
    State(state): State<AppState>,
    mut flash: Flash,
) -> (Flash, AdminTemplate) {
    let invite_link = extract_invite_link(&mut flash);
    let auth_user = PageAuthUser {
        user: admin.user.clone(),
        session: admin.session.clone(),
        via_forward_auth: admin.via_forward_auth,
        csrf_token: admin.csrf_token.clone(),
    };
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    let original_admin_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    let effective_admin_id = admin.user.id;

    let users = crate::models::user::list_all(&state.db)
        .await
        .ok()
        .unwrap_or_default()
        .into_iter()
        .map(|u| {
            let disabled = u.is_disabled();
            AdminUserView {
                id: u.id,
                username: u.username,
                role: u.role.as_str().to_string(),
                disabled,
                created_at: u.created_at.format("%Y-%m-%d").to_string(),
                created_at_iso: u.created_at.to_rfc3339(),
                is_self: u.id == effective_admin_id || u.id == original_admin_id,
                // `"!"` marks an account that never set a password.
                awaiting_password: u.password_hash == "!",
                invite_expires_at: None,
            }
        })
        .collect();

    // Per-row lookup is fine: self-hosted instances have few accounts.
    let mut users: Vec<AdminUserView> = users;
    for row in &mut users {
        row.invite_expires_at = crate::models::user_invite::find_live_for_user(&state.db, row.id)
            .await
            .ok()
            .flatten()
            .map(|invite| invite.expires_at.format("%Y-%m-%d %H:%M").to_string());
    }

    let can_create_account = crate::models::user::count(&state.db)
        .await
        .is_ok_and(|count| state.config.can_create_account(count));

    // Must mirror `handlers::admin::require_recent_authentication`.
    let needs_reauth = !admin.via_forward_auth
        && !admin.session.authenticated_recently(chrono::Utc::now())
        && !state.config.disable_local_auth;

    (
        flash,
        AdminTemplate {
            title: "Admin Panel",
            git_version: crate::GIT_VERSION,
            layout,
            csrf_token: auth_user.csrf_token.clone(),
            users,
            can_create_account,
            needs_reauth,
            invite_link,
        },
    )
}

/// One row of the SSR `/admin` user table.
pub struct AdminUserView {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub disabled: bool,
    pub created_at: String,
    pub created_at_iso: String,
    pub is_self: bool,
    /// No password set yet (invite not redeemed).
    pub awaiting_password: bool,
    /// Expiry of the outstanding invite; the link itself is unrecoverable.
    pub invite_expires_at: Option<String>,
}

/// Per-route template for `/admin`.
#[derive(Template)]
#[template(path = "admin.html")]
pub struct AdminTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`].
    pub csrf_token: String,
    pub users: Vec<AdminUserView>,
    /// One-time invite link from the last create/reissue; shown once only.
    pub invite_link: Option<String>,
    pub can_create_account: bool,
    /// Show the re-auth form up front, before any destructive action.
    pub needs_reauth: bool,
}
