use std::net::{IpAddr, SocketAddr};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::HeaderMap,
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use time::Duration;

use crate::AppState;
use crate::config::Config;
use crate::error::AppResult;
use crate::middleware::{FlashRedirect, SESSION_COOKIE_NAME};
use crate::models::user::{self, Role};
use crate::models::{category, session};

/// Path prefixes that must never trigger forward-auth auto-login: machine
/// endpoints (`GReader` native clients, JSON/passkey APIs, SSE, static assets)
/// authenticate by their own means.
const SKIP_PREFIXES: &[&str] = &[
    "/api",
    "/reader",
    "/accounts",
    "/events",
    "/static",
    "/favicon",
    "/health",
];

/// Parse a comma-separated groups header into trimmed, non-empty names.
pub fn parse_groups(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(std::string::ToString::to_string)
        .collect()
}

/// Map groups to a role: `Admin` iff `admin_group` is present, else `User`.
pub fn role_from_groups(groups: &[String], admin_group: &str) -> Role {
    if groups.iter().any(|g| g == admin_group) {
        Role::Admin
    } else {
        Role::User
    }
}

/// The identity supplied by a trusted forward-auth proxy on this request, if
/// any. Returns `None` when the feature is off, the peer IP is missing or not
/// in `TRUSTED_PROXY_NETWORKS`, or the identity header is absent/empty. Shared
/// by the middleware and the `AuthUser`/`PageAuthUser` extractors so the
/// trust logic lives in one place.
pub fn forward_auth_identity(
    config: &Config,
    peer_ip: Option<IpAddr>,
    headers: &HeaderMap,
) -> Option<String> {
    if !config.auth_proxy_enabled() {
        return None;
    }
    let ip = peer_ip?;
    if !config.is_trusted_peer(ip) {
        return None;
    }
    headers
        .get(config.auth_proxy_header.as_str())
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub async fn forward_auth(
    State(state): State<AppState>,
    jar: CookieJar,
    req: Request,
    next: Next,
) -> Response {
    let config = &state.config;

    // Feature off → nothing to do.
    if !config.auth_proxy_enabled() {
        return next.run(req).await;
    }

    // Only engage for browser page routes; skip before any DB work so
    // API/static requests with a session cookie don't pay a pointless lookup.
    if SKIP_PREFIXES
        .iter()
        .any(|p| req.uri().path().starts_with(p))
    {
        return next.run(req).await;
    }

    // Already carrying a VALID (non-expired) session → leave it to the normal
    // flow. A present-but-invalid cookie (e.g. after logout or expiry) must NOT
    // block forward-auth, or the user is locked out.
    if let Some(token) = jar.get(SESSION_COOKIE_NAME).map(|c| c.value().to_string()) {
        let valid = session::find_by_token(&state.db, &token)
            .await
            .is_ok_and(|s| s.is_some_and(|s| !s.is_expired()));
        if valid {
            return next.run(req).await;
        }
    }

    // Trusted-peer + identity-header check (shared with the auth extractors).
    let peer_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    let Some(username) = forward_auth_identity(config, peer_ip, req.headers()) else {
        return next.run(req).await;
    };

    // Optional group → role mapping (recomputed on every login when enabled).
    let desired_role = if config.group_mapping_enabled() {
        let groups = req
            .headers()
            .get(config.auth_proxy_groups_header.as_str())
            .and_then(|v| v.to_str().ok())
            .map(parse_groups)
            .unwrap_or_default();
        Some(role_from_groups(&groups, &config.auth_proxy_admin_group))
    } else {
        None
    };

    let allow_creation = config.auth_proxy_user_creation;

    // Resolve (or JIT-create) the account and open a session. `None` means
    // "reject" (unknown user with creation off, or a disabled account).
    let outcome: AppResult<Option<String>> = async {
        let user = if let Some(u) = user::find_by_username(&state.db, &username).await? {
            if u.is_disabled() {
                return Ok(None);
            }
            if let Some(role) = desired_role
                && u.role != role
            {
                user::update_role(&state.db, u.id, role).await?;
            }
            u
        } else {
            if !allow_creation {
                return Ok(None);
            }
            let role = match desired_role {
                Some(r) => r,
                None if user::count(&state.db).await? == 0 => Role::Admin,
                None => Role::User,
            };
            // Sentinel hash never verifies, so local password login is
            // impossible for forward-auth-provisioned accounts.
            let created = user::create_user(&state.db, &username, "!", role).await?;
            category::create_category(&state.db, created.id, "Uncategorized").await?;
            created
        };
        let new_session = session::create_session(&state.db, user.id).await?;
        Ok(Some(new_session.session_token))
    }
    .await;

    let token = match outcome {
        Ok(Some(token)) => token,
        Ok(None) => {
            return FlashRedirect::warning(
                "/login",
                "You are not authorized to access this instance.",
            )
            .into_response();
        }
        // DB error → fail closed: fall back to the normal (cookie) flow.
        _ => return next.run(req).await,
    };

    let cookie = Cookie::build((SESSION_COOKIE_NAME, token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .max_age(Duration::days(session::SESSION_ABSOLUTE_MAX_DAYS))
        .build();

    // Redirect to the same URL; the just-set cookie authenticates the retry.
    let location = req
        .uri()
        .path_and_query()
        .map_or_else(|| "/".to_string(), |pq| pq.as_str().to_string());

    (jar.add(cookie), Redirect::to(&location)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::user::Role;

    #[test]
    fn test_parse_groups() {
        assert_eq!(
            parse_groups("admins, users ,, dev"),
            vec!["admins".to_string(), "users".to_string(), "dev".to_string()]
        );
        assert!(parse_groups("   ").is_empty());
    }

    #[test]
    fn test_role_from_groups() {
        let groups = vec!["users".to_string(), "admins".to_string()];
        assert_eq!(role_from_groups(&groups, "admins"), Role::Admin);
        assert_eq!(role_from_groups(&groups, "superadmins"), Role::User);
        assert_eq!(role_from_groups(&[], "admins"), Role::User);
    }
}
