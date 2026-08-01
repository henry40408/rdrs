use std::net::SocketAddr;

use axum::{
    Json,
    extract::{ConnectInfo, Extension, Path, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use webauthn_rs::prelude::*;
use webauthn_rs_proto::ResidentKeyRequirement;

use crate::AppState;
use crate::error::{AppError, AppResult};
use crate::middleware::{AuthUser, Bucket, RecentlyAuthenticated, build_session_cookie};
use crate::models::{passkey, session, user, webauthn_challenge};
use crate::services::audit;
use crate::utils::http::request_user_agent;

// --- Registration ---

#[derive(Debug, Serialize)]
pub struct StartRegistrationResponse {
    pub options: CreationChallengeResponse,
}

/// The re-authentication check lives here, on the *start* of the ceremony,
/// not on its finish.
///
/// Two reasons, and the first is fatal to the alternative: the challenge is
/// single-use (`find_and_delete_challenge`), so a 403 at the finish step would
/// consume it and leave the retry — after the user has typed their password —
/// with nothing to complete. The user would also have already touched their
/// authenticator, only to be asked for a password afterwards.
///
/// Checking only here is sufficient: a credential cannot be registered without
/// a challenge, a challenge only exists because this handler issued one, and
/// this handler will not issue one to a session that has not authenticated
/// recently.
pub async fn start_registration(
    State(state): State<AppState>,
    auth_user: RecentlyAuthenticated,
) -> AppResult<Json<StartRegistrationResponse>> {
    let user_id = auth_user.user.id;
    let username = auth_user.user.username.clone();

    let existing_passkeys = passkey::list_by_user(&state.db, user_id).await?;

    let exclude_credentials: Vec<CredentialID> = existing_passkeys
        .iter()
        .map(|p| CredentialID::from(p.credential_id.clone()))
        .collect();

    let user_uuid = Uuid::new_v4();
    let (mut ccr, reg_state) = state
        .webauthn
        .start_passkey_registration(user_uuid, &username, &username, Some(exclude_credentials))
        .map_err(|e| AppError::PasskeyRegistrationFailed(e.to_string()))?;

    // webauthn-rs asks for `residentKey: discouraged` here, which is wrong for
    // this app: sign-in is usernameless (`start_authentication` sends an empty
    // `allowCredentials`), so a credential the authenticator cannot discover on
    // its own could be registered and then never be usable to log in. Ask for a
    // discoverable credential instead, which is what a passkey is.
    //
    // Patched on the response rather than the builder because
    // `start_passkey_registration` hardcodes the flag and exposes no knob;
    // `finish_passkey_registration` does not re-read it, so this is the whole
    // change. `require_resident_key` is set alongside for WebAuthn L1 clients,
    // which never learned the newer field.
    if let Some(selection) = ccr.public_key.authenticator_selection.as_mut() {
        selection.resident_key = Some(ResidentKeyRequirement::Required);
        selection.require_resident_key = true;
    }

    // Serialize and store the registration state
    let state_json =
        serde_json::to_string(&reg_state).map_err(|e| AppError::Internal(e.to_string()))?;
    let challenge_bytes: Vec<u8> = ccr.public_key.challenge.as_ref().to_vec();

    webauthn_challenge::create_challenge(
        &state.db,
        &challenge_bytes,
        Some(user_id),
        webauthn_challenge::ChallengeType::Registration,
        &state_json,
    )
    .await?;

    Ok(Json(StartRegistrationResponse { options: ccr }))
}

#[derive(Debug, Deserialize)]
pub struct FinishRegistrationRequest {
    pub name: String,
    pub credential: RegisterPublicKeyCredential,
}

#[derive(Debug, Serialize)]
pub struct FinishRegistrationResponse {
    pub id: i64,
    pub name: String,
}

/// Deliberately takes a plain [`AuthUser`]: the freshness check happened at
/// `start_registration`, and repeating it here would fail a ceremony that
/// merely straddled the window boundary — after the challenge was already
/// spent. See that handler for why the start is the right place.
pub async fn finish_registration(
    State(state): State<AppState>,
    auth_user: AuthUser,
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(req): Json<FinishRegistrationRequest>,
) -> AppResult<(StatusCode, Json<FinishRegistrationResponse>)> {
    if req.name.is_empty() {
        return Err(AppError::Validation("Passkey name is required".to_string()));
    }

    let user_id = auth_user.user.id;

    // Find and consume the challenge
    let challenge = webauthn_challenge::find_and_delete_challenge(
        &state.db,
        Some(user_id),
        webauthn_challenge::ChallengeType::Registration,
    )
    .await?;

    // Deserialize the registration state
    let reg_state: PasskeyRegistration = serde_json::from_str(&challenge.state_data)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Complete registration
    let passkey_data = state
        .webauthn
        .finish_passkey_registration(&req.credential, &reg_state)
        .map_err(|e| AppError::PasskeyRegistrationFailed(e.to_string()))?;

    // Serialize the passkey data for storage
    let credential_id: Vec<u8> = passkey_data.cred_id().as_ref().to_vec();
    let public_key_json =
        serde_json::to_vec(&passkey_data).map_err(|e| AppError::Internal(e.to_string()))?;

    // Get transports from the credential response if available
    let transports = req.credential.response.transports.as_ref().map(|t| {
        t.iter()
            .map(|t| format!("{t:?}").to_lowercase())
            .collect::<Vec<_>>()
            .join(",")
    });

    let name = req.name;
    let new_passkey = passkey::create_passkey(
        &state.db,
        user_id,
        &credential_id,
        &public_key_json,
        0,
        &name,
        transports.as_deref(),
    )
    .await?;

    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    audit::passkey_registered(
        &state.config.secret,
        &auth_user.session.session_token,
        user_id,
        new_passkey.id,
        &new_passkey.name,
        &state.config.client_ip(peer, &headers).to_string(),
        &request_user_agent(&headers),
    );

    Ok((
        StatusCode::CREATED,
        Json(FinishRegistrationResponse {
            id: new_passkey.id,
            name: new_passkey.name,
        }),
    ))
}

// --- Authentication ---

#[derive(Debug, Serialize)]
pub struct StartAuthenticationResponse {
    pub options: RequestChallengeResponse,
}

/// Issue a usernameless sign-in challenge.
///
/// The challenge carries **no `allowCredentials`**. An earlier version built
/// it with `start_passkey_authentication` over every row in the `passkey`
/// table, which does the opposite of what its comment claimed: that call
/// populates `allowCredentials` with each credential it is given, so a single
/// unauthenticated request returned the credential ID of every passkey on the
/// instance — stable, linkable per-user identifiers, plus a count of how many
/// accounts had enrolled one — to anyone who asked.
///
/// Nothing here reads the database any more, which also retires the
/// account-existence oracle that motivated the rate limit below: the response
/// is now identical whether the instance has a thousand passkeys or none. The
/// budget is still charged, because each call writes a challenge row and
/// unauthenticated writes should not be free.
pub async fn start_authentication(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
) -> AppResult<Json<StartAuthenticationResponse>> {
    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let ip = state.config.client_ip(peer, &headers);
    if let Some(retry_after_secs) = state
        .login_rate_limiter
        .try_acquire(Bucket::PasskeyProbe, ip)
        .retry_after_secs()
    {
        tracing::warn!(event = "auth.rate_limited", %ip, bucket = ?Bucket::PasskeyProbe, endpoint = "POST /api/passkey/auth/start", "credential attempt rate limited");
        audit::login_rate_limited(
            "POST /api/passkey/auth/start",
            "passkey_probe",
            &ip.to_string(),
        );
        return Err(AppError::TooManyRequests { retry_after_secs });
    }

    let (mut rcr, auth_state) = state
        .webauthn
        .start_discoverable_authentication()
        .map_err(|e| AppError::PasskeyAuthenticationFailed(e.to_string()))?;

    // That call stamps `mediation: conditional`, which belongs to the browser
    // autofill flow: a conditional `navigator.credentials.get()` waits silently
    // for the user to pick a passkey from an input's dropdown. rdrs drives this
    // from an explicit "Login with Passkey" button and wants the modal, so the
    // field is cleared rather than advertising a mode `login.js` does not honour.
    rcr.mediation = None;

    // Store the auth state
    let state_json =
        serde_json::to_string(&auth_state).map_err(|e| AppError::Internal(e.to_string()))?;
    let challenge_bytes: Vec<u8> = rcr.public_key.challenge.as_ref().to_vec();

    webauthn_challenge::create_challenge(
        &state.db,
        &challenge_bytes,
        None, // No user_id for authentication
        webauthn_challenge::ChallengeType::Authentication,
        &state_json,
    )
    .await?;

    Ok(Json(StartAuthenticationResponse { options: rcr }))
}

#[derive(Debug, Deserialize)]
pub struct FinishAuthenticationRequest {
    pub credential: PublicKeyCredential,
}

#[derive(Debug, Serialize)]
pub struct FinishAuthenticationResponse {
    pub id: i64,
    pub username: String,
}

pub async fn finish_authentication(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(req): Json<FinishAuthenticationRequest>,
) -> AppResult<(CookieJar, Json<FinishAuthenticationResponse>)> {
    // Reserve an attempt before the challenge lookup or WebAuthn signature
    // verification — same ordering rationale as password login: the check
    // must run before any work an attacker's guess could otherwise spend.
    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let ip = state.config.client_ip(peer, &headers);
    if let Some(retry_after_secs) = state
        .login_rate_limiter
        .try_acquire(Bucket::Login, ip)
        .retry_after_secs()
    {
        tracing::warn!(event = "auth.rate_limited", %ip, bucket = ?Bucket::Login, endpoint = "POST /api/passkey/auth/finish", "credential attempt rate limited");
        audit::login_rate_limited("POST /api/passkey/auth/finish", "login", &ip.to_string());
        return Err(AppError::TooManyRequests { retry_after_secs });
    }

    // Find and consume the challenge
    let challenge = webauthn_challenge::find_and_delete_challenge(
        &state.db,
        None,
        webauthn_challenge::ChallengeType::Authentication,
    )
    .await?;

    // Deserialize the auth state
    let auth_state: DiscoverableAuthentication = serde_json::from_str(&challenge.state_data)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Find the passkey by credential ID (use raw_id which contains raw bytes).
    // Client-supplied, and safe to trust for *selection* only: it decides which
    // stored public key the signature is checked against, and naming someone
    // else's credential just means the signature fails to verify below.
    let credential_id: Vec<u8> = req.credential.raw_id.as_ref().to_vec();
    let stored_passkey = passkey::find_by_credential_id(&state.db, &credential_id)
        .await?
        .ok_or(AppError::PasskeyNotFound)?;

    // Verify the user is not disabled
    let db_user = user::find_by_id(&state.db, stored_passkey.user_id)
        .await?
        .ok_or(AppError::UserNotFound)?;
    if db_user.is_disabled() {
        return Err(AppError::UserDisabled);
    }

    // Deserialize the stored passkey data
    let mut passkey_data: Passkey = serde_json::from_slice(&stored_passkey.public_key)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Complete authentication. The single stored credential resolved above is
    // the only one the assertion is allowed to match: `finish_discoverable_*`
    // installs it as the allow-list before verifying, so the challenge going
    // out empty costs nothing in strictness here.
    let auth_result = state
        .webauthn
        .finish_discoverable_authentication(
            &req.credential,
            auth_state,
            std::slice::from_ref(&DiscoverableKey::from(&passkey_data)),
        )
        .map_err(|e| AppError::PasskeyAuthenticationFailed(e.to_string()))?;

    // The WebAuthn ceremony verified successfully: hand the reservation back
    // before the session is created, so a legitimate user is never locked
    // out by their own successful passkey sign-ins.
    state.login_rate_limiter.release(Bucket::Login, ip);

    // Update the counter
    passkey_data.update_credential(&auth_result);
    let passkey_id = stored_passkey.id;
    let counter = auth_result.counter() as i64;
    let passkey_user_id = stored_passkey.user_id;

    passkey::update_counter(&state.db, passkey_id, counter).await?;
    let user_agent = request_user_agent(&headers);
    let ip = ip.to_string();
    let new_session = session::create_session(&state.db, passkey_user_id, &user_agent, &ip).await?;
    audit::session_created(
        &state.config.secret,
        &new_session.session_token,
        passkey_user_id,
        "passkey",
        &ip,
        &user_agent,
    );

    let cookie = build_session_cookie(
        &new_session.session_token,
        &state.config.secret,
        state.config.cookie_secure,
    );
    let csrf = crate::middleware::build_csrf_cookie(
        &new_session.session_token,
        &state.config.secret,
        state.config.cookie_secure,
    );

    Ok((
        jar.add(cookie).add(csrf),
        Json(FinishAuthenticationResponse {
            id: db_user.id,
            username: db_user.username,
        }),
    ))
}

// --- Management ---

#[derive(Debug, Serialize)]
pub struct PasskeyInfo {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ListPasskeysResponse {
    pub passkeys: Vec<PasskeyInfo>,
}

pub async fn list_passkeys(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<ListPasskeysResponse>> {
    let user_id = auth_user.user.id;
    let passkeys = passkey::list_by_user(&state.db, user_id).await?;

    let passkey_infos: Vec<PasskeyInfo> = passkeys
        .into_iter()
        .map(|p| PasskeyInfo {
            id: p.id,
            name: p.name,
            created_at: p.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
            last_used_at: p
                .last_used_at
                .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()),
        })
        .collect();

    Ok(Json(ListPasskeysResponse {
        passkeys: passkey_infos,
    }))
}

#[derive(Debug, Deserialize)]
pub struct RenamePasskeyRequest {
    pub name: String,
}

pub async fn rename_passkey(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<RenamePasskeyRequest>,
) -> AppResult<StatusCode> {
    if req.name.is_empty() {
        return Err(AppError::Validation("Name is required".to_string()));
    }

    let user_id = auth_user.user.id;
    let name = req.name;
    passkey::rename_passkey(&state.db, id, user_id, &name).await?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_passkey(
    State(state): State<AppState>,
    auth_user: RecentlyAuthenticated,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let user_id = auth_user.user.id;
    passkey::delete_passkey(&state.db, id, user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
