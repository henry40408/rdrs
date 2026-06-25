use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Request, State},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use time::Duration;

use crate::error::AppError;
use crate::middleware::{FlashRedirect, SESSION_COOKIE_NAME};
use crate::models::user::{self, Role};
use crate::models::{category, session};
use crate::AppState;

/// Path prefixes that must never trigger forward-auth auto-login: machine
/// endpoints (GReader native clients, JSON/passkey APIs, SSE, static assets)
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
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
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

pub async fn forward_auth(
    State(state): State<AppState>,
    jar: CookieJar,
    req: Request,
    next: Next,
) -> Response {
    let config = &state.config;

    // Feature off, or already carrying a session cookie → nothing to do.
    if !config.auth_proxy_enabled() || jar.get(SESSION_COOKIE_NAME).is_some() {
        return next.run(req).await;
    }

    // Only engage for browser page routes.
    if SKIP_PREFIXES
        .iter()
        .any(|p| req.uri().path().starts_with(p))
    {
        return next.run(req).await;
    }

    // Fail closed: without a known peer IP we cannot trust the header.
    let Some(peer_ip) = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
    else {
        return next.run(req).await;
    };
    if !config.is_trusted_peer(peer_ip) {
        return next.run(req).await;
    }

    // Read the identity header.
    let Some(username) = req
        .headers()
        .get(config.auth_proxy_header.as_str())
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
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
    let outcome = state
        .db
        .user(move |conn| {
            let user = match user::find_by_username(conn, &username)? {
                Some(u) => {
                    if u.is_disabled() {
                        return Ok::<Option<String>, AppError>(None);
                    }
                    if let Some(role) = desired_role {
                        if u.role != role {
                            user::update_role(conn, u.id, role)?;
                        }
                    }
                    u
                }
                None => {
                    if !allow_creation {
                        return Ok(None);
                    }
                    let role = match desired_role {
                        Some(r) => r,
                        None if user::count(conn)? == 0 => Role::Admin,
                        None => Role::User,
                    };
                    // Sentinel hash never verifies, so local password login is
                    // impossible for forward-auth-provisioned accounts.
                    let created = user::create_user(conn, &username, "!", role)?;
                    category::create_category(conn, created.id, "Uncategorized")?;
                    created
                }
            };
            let new_session = session::create_session(conn, user.id)?;
            Ok(Some(new_session.session_token))
        })
        .await;

    let token = match outcome {
        Ok(Ok(Some(token))) => token,
        Ok(Ok(None)) => {
            return FlashRedirect::warning(
                "/login",
                "You are not authorized to access this instance.",
            )
            .into_response();
        }
        // DB/join error → fail closed: fall back to the normal (cookie) flow.
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
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

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
