use askama::Template;
use axum::extract::State;

use super::{AppLayoutContext, build_app_layout};
use crate::AppState;
use crate::middleware::auth::{PageAdminUser, PageAuthUser};
use crate::middleware::flash::Flash;

/// `GET /settings`. Admin-only: it exposes deployment internals.
pub async fn settings_page(
    admin: PageAdminUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, SettingsTemplate) {
    let auth_user = PageAuthUser {
        user: admin.user.clone(),
        session: admin.session.clone(),
        via_forward_auth: admin.via_forward_auth,
        csrf_token: admin.csrf_token.clone(),
    };
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let user_agent_is_default = state.config.user_agent == crate::config::DEFAULT_USER_AGENT;

    (
        flash,
        SettingsTemplate {
            title: "App",
            git_version: crate::GIT_VERSION,
            layout,
            database_url: crate::config::redact_database_url(&state.config.database_url),
            server_bind: state.config.server_bind,
            user_agent: state.config.user_agent.clone(),
            user_agent_is_default,
            multi_user_enabled: state.config.multi_user_enabled,
            secret_generated: state.config.secret_generated,
            webauthn_rp_id: state.config.webauthn_rp_id.clone(),
            webauthn_rp_origin: state.config.webauthn_rp_origin.clone(),
            webauthn_rp_name: state.config.webauthn_rp_name.clone(),
            auth_proxy_header: state.config.auth_proxy_header.clone(),
            trusted_proxy_networks: state
                .config
                .trusted_proxy_networks
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            auth_proxy_user_creation: state.config.auth_proxy_user_creation,
            auth_proxy_groups_header: state.config.auth_proxy_groups_header.clone(),
            auth_proxy_admin_group: state.config.auth_proxy_admin_group.clone(),
            disable_local_auth: state.config.disable_local_auth,
            auth_proxy_logout_url: state
                .config
                .auth_proxy_logout_url
                .clone()
                .unwrap_or_default(),
        },
    )
}

/// `/settings` server-config table. `git_version` duplicates `layout`'s because
/// `base.html` references it outside the layout's blocks.
#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub database_url: String,
    pub server_bind: std::net::SocketAddr,
    pub user_agent: String,
    pub user_agent_is_default: bool,
    pub multi_user_enabled: bool,
    pub secret_generated: bool,
    pub webauthn_rp_id: String,
    pub webauthn_rp_origin: String,
    pub webauthn_rp_name: String,
    pub auth_proxy_header: String,
    pub trusted_proxy_networks: String,
    pub auth_proxy_user_creation: bool,
    pub auth_proxy_groups_header: String,
    pub auth_proxy_admin_group: String,
    pub disable_local_auth: bool,
    pub auth_proxy_logout_url: String,
}
