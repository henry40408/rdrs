use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde::{Deserialize, Serialize};
use time::Duration;
use uuid::Uuid;
use webauthn_rs::prelude::*;

use crate::error::{AppError, AppResult};
use crate::middleware::{AuthUser, SESSION_COOKIE_NAME};
use crate::models::{passkey, session, user, webauthn_challenge};
use crate::AppState;

// --- Registration ---

#[derive(Debug, Serialize)]
pub struct StartRegistrationResponse {
    pub options: CreationChallengeResponse,
}

pub async fn start_registration(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<StartRegistrationResponse>> {
    let user_id = auth_user.user.id;
    let username = auth_user.user.username.clone();

    let existing_passkeys = state
        .db
        .user(move |conn| passkey::list_by_user(conn, user_id))
        .await??;

    let exclude_credentials: Vec<CredentialID> = existing_passkeys
        .iter()
        .map(|p| CredentialID::from(p.credential_id.clone()))
        .collect();

    let user_uuid = Uuid::new_v4();
    let (ccr, reg_state) = state
        .webauthn
        .start_passkey_registration(user_uuid, &username, &username, Some(exclude_credentials))
        .map_err(|e| AppError::PasskeyRegistrationFailed(e.to_string()))?;

    // Serialize and store the registration state
    let state_json =
        serde_json::to_string(&reg_state).map_err(|e| AppError::Internal(e.to_string()))?;
    let challenge_bytes: Vec<u8> = ccr.public_key.challenge.as_ref().to_vec();

    state
        .db
        .user(move |conn| {
            webauthn_challenge::create_challenge(
                conn,
                &challenge_bytes,
                Some(user_id),
                webauthn_challenge::ChallengeType::Registration,
                &state_json,
            )
        })
        .await??;

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

pub async fn finish_registration(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Json(req): Json<FinishRegistrationRequest>,
) -> AppResult<(StatusCode, Json<FinishRegistrationResponse>)> {
    if req.name.is_empty() {
        return Err(AppError::Validation("Passkey name is required".to_string()));
    }

    let user_id = auth_user.user.id;

    // Find and consume the challenge
    let challenge = state
        .db
        .user(move |conn| {
            webauthn_challenge::find_and_delete_challenge(
                conn,
                Some(user_id),
                webauthn_challenge::ChallengeType::Registration,
            )
        })
        .await??;

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
            .map(|t| format!("{:?}", t).to_lowercase())
            .collect::<Vec<_>>()
            .join(",")
    });

    let name = req.name;
    let new_passkey = state
        .db
        .user(move |conn| {
            passkey::create_passkey(
                conn,
                user_id,
                &credential_id,
                &public_key_json,
                0,
                &name,
                transports.as_deref(),
            )
        })
        .await??;

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

pub async fn start_authentication(
    State(state): State<AppState>,
) -> AppResult<Json<StartAuthenticationResponse>> {
    let all_passkeys = state.db.user(passkey::get_all_passkeys).await??;

    if all_passkeys.is_empty() {
        return Err(AppError::PasskeyAuthenticationFailed(
            "No passkeys registered".to_string(),
        ));
    }

    // Deserialize passkeys
    let passkey_credentials: Vec<Passkey> = all_passkeys
        .iter()
        .filter_map(|p| serde_json::from_slice(&p.public_key).ok())
        .collect();

    if passkey_credentials.is_empty() {
        return Err(AppError::PasskeyAuthenticationFailed(
            "No valid passkeys found".to_string(),
        ));
    }

    // Start authentication - use discoverable credentials (no allowCredentials)
    let (rcr, auth_state) = state
        .webauthn
        .start_passkey_authentication(&passkey_credentials)
        .map_err(|e| AppError::PasskeyAuthenticationFailed(e.to_string()))?;

    // Store the auth state
    let state_json =
        serde_json::to_string(&auth_state).map_err(|e| AppError::Internal(e.to_string()))?;
    let challenge_bytes: Vec<u8> = rcr.public_key.challenge.as_ref().to_vec();

    state
        .db
        .user(move |conn| {
            webauthn_challenge::create_challenge(
                conn,
                &challenge_bytes,
                None, // No user_id for authentication
                webauthn_challenge::ChallengeType::Authentication,
                &state_json,
            )
        })
        .await??;

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
    Json(req): Json<FinishAuthenticationRequest>,
) -> AppResult<(CookieJar, Json<FinishAuthenticationResponse>)> {
    // Find and consume the challenge
    let challenge = state
        .db
        .user(|conn| {
            webauthn_challenge::find_and_delete_challenge(
                conn,
                None,
                webauthn_challenge::ChallengeType::Authentication,
            )
        })
        .await??;

    // Deserialize the auth state
    let auth_state: PasskeyAuthentication = serde_json::from_str(&challenge.state_data)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Find the passkey by credential ID (use raw_id which contains raw bytes)
    let credential_id: Vec<u8> = req.credential.raw_id.as_ref().to_vec();
    let (stored_passkey, db_user) = state
        .db
        .user(move |conn| {
            let stored_passkey = passkey::find_by_credential_id(conn, &credential_id)?
                .ok_or(AppError::PasskeyNotFound)?;

            // Verify the user is not disabled
            let db_user =
                user::find_by_id(conn, stored_passkey.user_id)?.ok_or(AppError::UserNotFound)?;
            if db_user.is_disabled() {
                return Err(AppError::UserDisabled);
            }

            Ok::<_, AppError>((stored_passkey, db_user))
        })
        .await??;

    // Deserialize the stored passkey data
    let mut passkey_data: Passkey = serde_json::from_slice(&stored_passkey.public_key)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Complete authentication
    let auth_result = state
        .webauthn
        .finish_passkey_authentication(&req.credential, &auth_state)
        .map_err(|e| AppError::PasskeyAuthenticationFailed(e.to_string()))?;

    // Update the counter
    passkey_data.update_credential(&auth_result);
    let passkey_id = stored_passkey.id;
    let counter = auth_result.counter() as i64;
    let passkey_user_id = stored_passkey.user_id;

    let new_session = state
        .db
        .user(move |conn| {
            passkey::update_counter(conn, passkey_id, counter)?;
            let new_session = session::create_session(conn, passkey_user_id)?;
            Ok::<_, AppError>(new_session)
        })
        .await??;

    let cookie = Cookie::build((SESSION_COOKIE_NAME, new_session.session_token))
        .path("/")
        .http_only(true)
        .same_site(axum_extra::extract::cookie::SameSite::Lax)
        .max_age(Duration::days(session::SESSION_EXPIRY_DAYS))
        .build();

    Ok((
        jar.add(cookie),
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
    let passkeys = state
        .db
        .user(move |conn| passkey::list_by_user(conn, user_id))
        .await??;

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
    state
        .db
        .user(move |conn| passkey::rename_passkey(conn, id, user_id, &name))
        .await??;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_passkey(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let user_id = auth_user.user.id;
    state
        .db
        .user(move |conn| passkey::delete_passkey(conn, id, user_id))
        .await??;

    Ok(StatusCode::NO_CONTENT)
}
