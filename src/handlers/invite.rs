//! Redeeming a one-time link: the only way an account gets its first password.
//!
//! Everything here is anonymous — the token in the URL *is* the authority — so
//! each failing case has to answer identically. An unknown token, an expired
//! one and one that was already spent all render the same page and reveal no
//! username, or the endpoint would become the account oracle that removing
//! self-service registration was meant to close.

use std::net::SocketAddr;

use axum::{
    Form,
    extract::{ConnectInfo, Extension, Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
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

/// Everything the redemption page needs once a token has been accepted.
struct LiveInvite {
    invite: crate::models::user_invite::UserInvite,
    username: String,
}

/// Resolve a token to a live invite and the account it belongs to.
///
/// `None` covers every failure identically — unknown token, expired, already
/// consumed, or an account that has since been deleted. Callers must not
/// distinguish them in what they render.
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
pub async fn invite_page(State(state): State<AppState>, Path(token): Path<String>) -> Response {
    let Some(live) = resolve(&state, &token).await else {
        return InviteTemplate::invalid().into_response();
    };

    // The username is shown only once the token has been accepted. Holding a
    // valid link already authorises knowing whose account it opens; showing it
    // before that would answer questions about accounts to anyone who guessed
    // a URL.
    InviteTemplate::form(&token, live.username).into_response()
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
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    Path(token): Path<String>,
    Form(req): Form<RedeemForm>,
) -> Response {
    // Throttled before the token is even looked up: this endpoint runs the
    // strength estimator and Argon2, and it is reachable without a session, so
    // it is the cheapest anonymous way to spend server CPU in the app.
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
        return InviteTemplate::error(&token, live.username, "Passwords do not match.")
            .into_response();
    }

    if let Err(AppError::Validation(msg)) =
        validate_password_strength(&req.password, &[&live.username])
    {
        return InviteTemplate::error(&token, live.username, &msg).into_response();
    }

    let Ok(password_hash) = hash_password(&req.password) else {
        return InviteTemplate::error(&token, live.username, "Could not set the password.")
            .into_response();
    };

    // Spend the link *before* writing the password. Two submissions racing on
    // one token both pass `resolve` above; only the caller that wins this
    // update may go on to write, or the loser would silently overwrite the
    // password the winner just chose. Losing looks exactly like arriving at a
    // spent link, which is what it is.
    match user_invite::consume(&state.db, live.invite.id).await {
        Ok(true) => {}
        _ => return InviteTemplate::invalid().into_response(),
    }

    if user::update_password(&state.db, live.invite.user_id, &password_hash)
        .await
        .is_err()
    {
        return InviteTemplate::error(&token, live.username, "Could not set the password.")
            .into_response();
    }

    // An admin-issued reset lands here too, and the account may well have live
    // sessions and API tokens from before. Clearing both matches what changing
    // a password from the settings page does: the point of a reset is that
    // whatever came before stops working.
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
