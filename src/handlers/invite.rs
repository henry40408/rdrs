//! Redeeming a one-time invite link. Anonymous — the token is the authority —
//! so every failure (unknown, expired, spent) must render identically and
//! reveal no username, or this becomes an account oracle.

use std::net::SocketAddr;

use axum::{
    Form,
    extract::{ConnectInfo, Extension, Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;
use serde::Deserialize;

use crate::AppState;
use crate::auth::{hash_password, validate_password_strength};
use crate::error::AppError;
use crate::handlers::pages::InviteTemplate;
use crate::middleware::Bucket;
use crate::middleware::flash::FlashRedirect;
use crate::models::{session, user, user_invite};
use crate::services::audit;
use crate::utils::http::request_user_agent;

struct LiveInvite {
    invite: crate::models::user_invite::UserInvite,
    username: String,
}

/// Resolve a token to a live invite. `None` covers every failure; callers must
/// not distinguish them in what they render.
async fn resolve(state: &AppState, token: &str) -> Option<LiveInvite> {
    let invite = user_invite::find_by_token(&state.db, &state.config.secret, token)
        .await
        .ok()
        .flatten()?;

    if !invite.is_live(chrono::Utc::now()) {
        return None;
    }

    let account = user::find_by_id(&state.db, invite.user_id).await.ok()??;

    Some(LiveInvite {
        invite,
        username: account.username,
    })
}

/// `GET /invite/{token}` — the "choose a password" form, or a dead end.
pub async fn invite_page(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(token): Path<String>,
) -> Response {
    let Some(live) = resolve(&state, &token).await else {
        return InviteTemplate::invalid().into_response();
    };

    // Username is shown only after the token is accepted.
    InviteTemplate::form(
        &token,
        live.username,
        crate::middleware::csrf_token_from_jar(&jar, &state.config.secret),
    )
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct RedeemForm {
    pub password: String,
    pub confirm_password: String,
}

/// `POST /invite/{token}` — set the password and spend the link.
pub async fn redeem_form(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    Path(token): Path<String>,
    Form(req): Form<RedeemForm>,
) -> Response {
    // Re-rendered on every failure below, so the retry carries a live token.
    let csrf_token = crate::middleware::csrf_token_from_jar(&jar, &state.config.secret);
    // Throttle before lookup: anonymous and runs zxcvbn + Argon2.
    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let ip = state.config.client_ip(peer, &headers);
    if let Some(retry_after_secs) = state
        .login_rate_limiter
        .try_acquire(Bucket::AccountSetup, ip)
        .retry_after_secs()
    {
        tracing::warn!(event = "auth.rate_limited", %ip, bucket = ?Bucket::AccountSetup, endpoint = "POST /invite", "credential attempt rate limited");
        audit::login_rate_limited("POST /invite", "invite_redeem", &ip.to_string());
        return InviteTemplate::throttled(retry_after_secs).into_response();
    }

    let Some(live) = resolve(&state, &token).await else {
        return InviteTemplate::invalid().into_response();
    };

    if req.password != req.confirm_password {
        return InviteTemplate::error(&token, live.username, "Passwords do not match.", csrf_token)
            .into_response();
    }

    if let Err(AppError::Validation(msg)) =
        validate_password_strength(&req.password, &[&live.username])
    {
        return InviteTemplate::error(&token, live.username, &msg, csrf_token).into_response();
    }

    let Ok(password_hash) = hash_password(&req.password) else {
        return InviteTemplate::error(
            &token,
            live.username,
            "Could not set the password.",
            csrf_token,
        )
        .into_response();
    };

    // Spend the link *before* writing the password so only the winner of a
    // racing double-submit writes; the loser sees a spent link.
    match user_invite::consume(&state.db, live.invite.id).await {
        Ok(true) => {}
        _ => return InviteTemplate::invalid().into_response(),
    }

    if user::update_password(&state.db, live.invite.user_id, &password_hash)
        .await
        .is_err()
    {
        return InviteTemplate::error(
            &token,
            live.username,
            "Could not set the password.",
            csrf_token,
        )
        .into_response();
    }

    // Admin resets land here too: revoke prior sessions and API tokens, as a
    // settings-page password change does.
    let _ = session::delete_user_sessions(&state.db, live.invite.user_id).await;
    let _ = crate::models::api_token::delete_user_tokens(&state.db, live.invite.user_id).await;

    audit::invite_consumed(
        &state.config.secret,
        &token,
        live.invite.user_id,
        &ip.to_string(),
        &request_user_agent(&headers),
    );

    FlashRedirect::success("/login", "Password set. You can sign in now.").into_response()
}
