use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, FromRequestParts, OptionalFromRequestParts, Request, State},
    http::{HeaderValue, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use chrono::Utc;
use time::Duration;

use crate::AppState;
use crate::error::AppError;
use crate::middleware::flash::FlashRedirect;
use crate::models::session::{self, Session};
use crate::models::user::{self, User};
use crate::services::audit;

pub const SESSION_COOKIE_NAME: &str = "session_token";

/// `__Host-`-prefixed session cookie name, used only when `Secure` is in
/// effect (see [`session_cookie_name`]).
///
/// OWASP's Session Management Cheat Sheet recommends the `__Host-` prefix for
/// the session cookie whenever possible: a browser enforces that a cookie
/// under this name is `Secure`, carries `Path=/`, has **no** `Domain`
/// attribute, and was set by this exact host — closing the channel by which a
/// sibling subdomain (or a same-site attacker who cannot touch this exact
/// host) could otherwise *write* a cookie that shadows ours.
///
/// This is defence in depth, not a fix for a real gap: session-cookie
/// fixation is already impossible in rdrs, because every cookie value carries
/// an HMAC signature (see [`crate::secret::sign_session`]) that
/// [`session_token_from_jar`] verifies before any database lookup — a
/// malicious subdomain cannot mint a cookie value that verifies, prefix or
/// no prefix. Adopting the prefix closes the OWASP checklist item and adds a
/// browser-enforced backstop, but no exploitable vulnerability existed
/// beforehand.
///
/// A `__Host-` cookie without `Secure` is silently *discarded* by the
/// browser, so this name must never be written while `Secure` is off — that
/// would make login silently impossible on a plain-HTTP deployment (rdrs's
/// own default). Hence it is only ever selected via [`session_cookie_name`].
pub const SESSION_COOKIE_NAME_HOST: &str = "__Host-session_token";

/// Which cookie name to *write* for the session cookie, given whether
/// `Secure` is in effect. See [`SESSION_COOKIE_NAME_HOST`] for why the
/// `__Host-` prefix cannot be used unconditionally.
pub fn session_cookie_name(secure: bool) -> &'static str {
    if secure {
        SESSION_COOKIE_NAME_HOST
    } else {
        SESSION_COOKIE_NAME
    }
}

/// Build the session cookie carrying `token`.
///
/// Every login path (password, passkey, forward-auth) goes through here so the
/// attributes cannot drift apart between them. The cookie *value* is the token
/// plus an HMAC signature (`<token>.<hmac>`, see [`crate::secret::sign_session`]),
/// so a tampered or forged cookie is rejected before any database lookup, and a
/// leaked `session.session_token` is not usable without the root key. `secure`
/// comes from [`crate::config::Config::cookie_secure`], which is derived from
/// `RDRS_PUBLIC_BASE_URL`'s scheme, and also picks the cookie *name* via
/// [`session_cookie_name`] — `Path=/` and no `Domain` (both already true
/// below) are the other two `__Host-` requirements, so whenever `secure` is
/// on this cookie is fully prefix-compliant.
pub fn build_session_cookie(token: &str, secret: &[u8], secure: bool) -> Cookie<'static> {
    Cookie::build((
        session_cookie_name(secure),
        crate::secret::sign_session(secret, token),
    ))
    .path("/")
    .http_only(true)
    .secure(secure)
    .same_site(SameSite::Lax)
    .max_age(Duration::days(session::SESSION_EXPIRY_DAYS))
    .build()
}

/// Read the signed session cookie from `jar` and return the database token it
/// carries, or `None` when the cookie is absent or its signature does not
/// verify. Every extractor and the forward-auth middleware funnel through here,
/// so the signature is always checked before the token reaches the database.
///
/// Tries [`SESSION_COOKIE_NAME_HOST`] first, then falls back to the
/// unprefixed [`SESSION_COOKIE_NAME`] — not only when the prefixed cookie is
/// *absent*, but also when it is present and fails to verify, before giving
/// up. Accepting the unprefixed name never weakens anything — forgery
/// resistance comes entirely from the HMAC signature verified below, not
/// from the cookie's name — and it is necessary: a server upgrade, or an
/// operator flipping `RDRS_COOKIE_SECURE`, must not silently log out every
/// existing session just because the cookie it is carrying no longer
/// matches the name the current config would mint. Falling through past a
/// present-but-invalid prefixed cookie (rather than stopping there) matters
/// too: a browser can easily be carrying a stale, empty `__Host-` cookie
/// left over from a logout's removal `Set-Cookie` (`Max-Age=0`, so a real
/// browser evicts it — but the moment before eviction, or a client that
/// mishandles expiry, could still send it) alongside a perfectly valid
/// unprefixed one; that must not shadow the valid cookie.
pub fn session_token_from_jar(jar: &CookieJar, secret: &[u8]) -> Option<String> {
    if let Some(token) = jar
        .get(SESSION_COOKIE_NAME_HOST)
        .and_then(|c| crate::secret::verify_session(secret, c.value()))
    {
        return Some(token);
    }
    let value = jar.get(SESSION_COOKIE_NAME)?.value().to_string();
    crate::secret::verify_session(secret, &value)
}

/// The channel by which an extractor asks [`slide_session_cookie`] to rotate
/// the session token.
///
/// The decision is made deep inside the request — in `AuthUser` /
/// `PageAuthUser`, where the sliding refresh already computes "this session is
/// due" — but the rotation itself must not happen there. An extractor sees
/// only request parts: it cannot know whether the response will be one a
/// cookie may ride on, and a rotation whose new token never reaches the client
/// would sign that client out as soon as the grace interval lapsed. So the
/// extractor raises the flag, and the middleware performs the rotation once it
/// has the finished response in hand and has ruled out the publicly cacheable
/// ones (`/api/feeds/{id}/icon` authenticates like any other route but is
/// served `public, max-age=…`).
#[derive(Clone, Default)]
pub struct RotationSlot(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl RotationSlot {
    /// Ask for the session token to be rotated on the way out. A no-op when the
    /// request carries no slot — i.e. on a route mounted outside this
    /// middleware, such as the SSE stream at `/events`, where nothing could
    /// deliver the new token anyway.
    pub fn request(parts: &Parts) {
        if let Some(slot) = parts.extensions.get::<Self>() {
            slot.0.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn requested(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Path prefixes skipped entirely: static assets, favicons, and the health
/// check must stay cacheable, and a `Set-Cookie` on any of them would poison
/// a shared cache sitting in front of the app. Deliberately narrower than
/// `ANON_SKIP_PREFIXES` in `csrf.rs` — `/api` and `/reader` are *not* skipped
/// here, since a pure-API or `GReader` client must still get its cookie's
/// `Max-Age` renewed.
///
/// This prefix list is not the whole defence: two `/api` routes (the image
/// proxy and the feed icon) are deliberately publicly cacheable and are
/// *not* listed here, since `/api` at large must still be slid. Those are
/// instead caught on the way out — see [`response_is_publicly_cacheable`].
const SLIDE_SKIP_PREFIXES: &[&str] = &["/static", "/favicon", "/health"];

/// Reissue the session and CSRF cookies on every authenticated request so
/// their `Max-Age` — now aligned with the sliding session TTL
/// (`SESSION_EXPIRY_DAYS`, 7 days, instead of the old 90-day absolute cap)
/// — keeps tracking "last used" the same way the database row's own
/// `expires_at` does, instead of logging out a browser that is still
/// actively in use.
///
/// Sliding the row's own `expires_at` remains `session::refresh_if_needed`'s
/// job, called from the `AuthUser`/`PageAuthUser` extractors. The one database
/// write this layer does perform is the token rotation those extractors ask
/// for (see [`RotationSlot`]), which has to happen here because only here is
/// the response — and therefore whether a new cookie can be delivered at all —
/// known. Otherwise, an absent or HMAC-invalid session cookie is passed
/// through untouched: `session_token_from_jar` already does the signature
/// check, so no unverified value is ever echoed back. A verified but row-less
/// *anonymous* session cookie (minted by
/// `anonymous_session` for a logged-out visitor) is slid the same way; that
/// is harmless and intentional, not a bug to "fix" later — it still expires
/// on its own schedule regardless of this middleware, and there is no
/// database row it could out-live.
///
/// Layered outside `anonymous_session` and inside `forward_auth` (see
/// `crate::create_router`), so it observes — and must never clobber — the
/// `Set-Cookie`s that layer and every handler below it emit. In particular,
/// `logout`'s empty removal cookies must reach the browser unmodified: for
/// each of the two *cookie purposes* (session, CSRF) — each of which may be
/// carried under either its unprefixed or `__Host-`-prefixed name — a
/// `Set-Cookie` already present under *either* name is left alone, and only
/// the absence of both causes a fresh one (refreshed `Max-Age`, and the same
/// value unless this request rotated the token, written under whichever name
/// `secure` selects) to be appended. Checking both names matters most for `logout`: it emits removal
/// cookies under all four names (see `handlers::auth::logout`), and if this
/// check only recognised the name it would itself write, it would happily
/// append a *live* cookie under the other name right next to a removal —
/// silently undoing the logout. See [`response_has_set_cookie_for_any`] for
/// the exact matching rule.
pub async fn slide_session_cookie(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    if SLIDE_SKIP_PREFIXES
        .iter()
        .any(|p| req.uri().path().starts_with(p))
    {
        return next.run(req).await;
    }

    let jar = CookieJar::from_headers(req.headers());
    let Some(token) = session_token_from_jar(&jar, &state.config.secret) else {
        return next.run(req).await;
    };

    let secret = &state.config.secret;
    let secure = state.config.cookie_secure;

    // Offer the extractors below a way to ask for a rotation, and keep our own
    // handle on the answer — see [`RotationSlot`].
    let slot = RotationSlot::default();
    req.extensions_mut().insert(slot.clone());

    let mut resp = next.run(req).await;

    // A live session cookie must never ride on a response a shared cache is
    // allowed to store — that cache would then hand this user's session to
    // the next visitor it serves. `handlers::proxy::proxy_image` and
    // `handlers::feed::get_feed_icon` deliberately mark their responses
    // `public, max-age=...` (the image proxy also passes through whatever
    // `Cache-Control` the upstream image server sent); skip the reissue
    // entirely for those instead of trying to enumerate their paths.
    //
    // Checked *before* the rotation below, not just before the reissue: a
    // rotation this response cannot carry would rename the session behind the
    // client's back and sign it out once the grace interval lapsed.
    if response_is_publicly_cacheable(&resp) {
        return resp;
    }

    // The rotation happens here rather than in the extractor that asked for it,
    // because only here is it known that the new token can reach the client.
    // `rotate_token` matches on the old token, so if a concurrent request beat
    // us to it we get `None` and keep using the token we have — still valid,
    // because the winner left it behind as the grace token.
    let token = if slot.requested() {
        match session::rotate_token(&state.db, &token).await {
            Ok(Some(rotated)) => {
                audit::session_token_rotated(secret, &token, &rotated);
                rotated
            }
            Ok(None) => token,
            Err(e) => {
                tracing::warn!(
                    event = "session.rotation_failed",
                    error = %e,
                    "session token rotation failed; keeping current token"
                );
                token
            }
        }
    } else {
        token
    };

    if !response_has_set_cookie_for_any(&resp, &[SESSION_COOKIE_NAME, SESSION_COOKIE_NAME_HOST]) {
        append_set_cookie(&mut resp, &build_session_cookie(&token, secret, secure));
    }
    if !response_has_set_cookie_for_any(
        &resp,
        &[
            crate::middleware::CSRF_COOKIE_NAME,
            crate::middleware::CSRF_COOKIE_NAME_HOST,
        ],
    ) {
        append_set_cookie(
            &mut resp,
            &crate::middleware::build_csrf_cookie(&token, secret, secure),
        );
    }

    resp
}

/// Whether `resp` already carries a `Set-Cookie` header for cookie `name`.
///
/// Every `Set-Cookie` value is split at its *first* `=`; the trimmed
/// substring before it is the cookie name, compared for an exact match. This
/// is deliberately not a substring search — a different cookie whose value
/// happens to contain `name` (e.g. `other=session_token_lookalike`) must not
/// count as a match.
fn response_has_set_cookie_for(resp: &Response, name: &str) -> bool {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| v.split_once('=').is_some_and(|(n, _)| n.trim() == name))
}

/// Whether `resp` already carries a `Set-Cookie` for *any* of `names`.
///
/// Used to check "is this cookie purpose already covered", where the purpose
/// (session, or CSRF) may be represented by either its unprefixed or
/// `__Host-`-prefixed name — see [`slide_session_cookie`].
fn response_has_set_cookie_for_any(resp: &Response, names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| response_has_set_cookie_for(resp, name))
}

/// Whether `resp` declares itself storable by a *shared* cache, in which
/// case a `Set-Cookie` must not be stapled to it. `no_store_for_authenticated`
/// (`middleware::cache_control`) is layered *inside* this middleware (see
/// `create_router`), so it has already run by the time a response reaches
/// here: an ordinary authenticated response already carries `Cache-Control:
/// no-store` from that inner layer, while the three deliberate
/// public-caching call sites (`handlers::proxy::proxy_image`,
/// `handlers::feed::get_feed_icon`) carry their own `public, max-age=...` —
/// or, for the proxy, whatever the upstream sent — untouched by that layer.
/// A response is "shared-cacheable" here whenever it carries a
/// `Cache-Control` header whose directives include neither `no-store` nor
/// `private`; an absent or non-UTF-8 header is treated as not cacheable, so
/// the default is to still slide the cookie.
fn response_is_publicly_cacheable(resp: &Response) -> bool {
    let Some(value) = resp.headers().get(header::CACHE_CONTROL) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    value.split(',').all(|directive| {
        let name = directive.split_once('=').map_or(directive, |(n, _)| n);
        let name = name.trim();
        !name.eq_ignore_ascii_case("no-store") && !name.eq_ignore_ascii_case("private")
    })
}

/// Append `cookie` as a `Set-Cookie` header on `resp`.
fn append_set_cookie(resp: &mut Response, cookie: &Cookie<'static>) {
    if let Ok(value) = HeaderValue::from_str(&cookie.to_string()) {
        resp.headers_mut().append(header::SET_COOKIE, value);
    }
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user: User,
    pub session: Session,
    pub via_forward_auth: bool,
}

/// An [`AuthUser`] whose session proved its credentials within
/// [`session::REAUTH_WINDOW_MINUTES`] — OWASP's "Reauthentication After Risk
/// Events", applied to the operations that change which credentials can open
/// the account.
///
/// It guards passkey registration and removal. Registering a passkey adds an
/// independently usable credential that a later password change will *not*
/// revoke (a password change ends every session and API token, but leaves
/// passkeys standing), so a session someone else picked up — an unlocked
/// laptop, a borrowed browser — must not be able to mint one silently.
///
/// Forward-auth sessions are exempt, and the exemption is not a gap: their
/// identity is asserted by the reverse proxy on every request, so rdrs has no
/// credential of its own to re-check, and the account may hold no usable
/// password at all (`forward_auth` writes a deliberately unverifiable hash for
/// accounts it creates). Requiring a password there would lock those users out
/// of passkey registration permanently rather than protecting them. Their
/// trust boundary is the proxy, and that is where a re-authentication policy
/// for them belongs.
///
/// Rejects with [`AppError::ReauthenticationRequired`], which the browser
/// turns into a password prompt and a retry.
#[derive(Debug, Clone)]
pub struct RecentlyAuthenticated {
    pub user: User,
    pub session: Session,
}

impl FromRequestParts<AppState> for RecentlyAuthenticated {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth_user = AuthUser::from_request_parts(parts, state).await?;
        if !auth_user.via_forward_auth && !auth_user.session.authenticated_recently(Utc::now()) {
            return Err(AppError::ReauthenticationRequired);
        }
        Ok(Self {
            user: auth_user.user,
            session: auth_user.session,
        })
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_request_parts(parts, state)
            .await
            .map_err(|_e| AppError::Unauthorized)?;

        let token =
            session_token_from_jar(&jar, &state.config.secret).ok_or(AppError::Unauthorized)?;

        let mut session = session::find_by_token(&state.db, &token)
            .await?
            .ok_or(AppError::Unauthorized)?;
        let expired = if session.is_expired() {
            session::delete_session(&state.db, &token).await?;
            audit::session_destroyed(&state.config.secret, &token, session.user_id, "expired");
            true
        } else {
            if let Some(new_expires_at) = session::refresh_if_needed(&state.db, &session).await? {
                audit::session_renewed(
                    &state.config.secret,
                    &token,
                    session.user_id,
                    new_expires_at,
                );
                session.expires_at = new_expires_at;
                // Same trigger, second effect: the session is due for a new
                // token as well. `slide_session_cookie` performs it on the way
                // out, where the response is known to be able to carry it.
                RotationSlot::request(parts);
            }
            let _ = session::touch_last_seen(&state.db, &session).await;
            false
        };

        if expired {
            return Err(AppError::Unauthorized);
        }

        let user_id = session.user_id;
        let user = user::find_by_id(&state.db, user_id)
            .await?
            .ok_or(AppError::Unauthorized)?;

        if user.is_disabled() {
            return Err(AppError::UserDisabled);
        }

        let peer_ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        let via_forward_auth = crate::middleware::forward_auth::forward_auth_identity(
            &state.config,
            peer_ip,
            &parts.headers,
        )
        .is_some();

        Ok(AuthUser {
            user,
            session,
            via_forward_auth,
        })
    }
}

/// Auth extractor for page routes that redirects to login on unauthorized
#[derive(Debug, Clone)]
pub struct PageAuthUser {
    pub user: User,
    pub session: Session,
    pub via_forward_auth: bool,
    /// The synchronizer token to render into this page's forms, so a POST
    /// submitted without JavaScript still satisfies `csrf_guard`. Derived from
    /// the *cookie* token rather than `session.session_token`: the guard
    /// re-derives from the cookie the browser sends back, and during a
    /// rotation's grace interval those two differ. See
    /// [`crate::middleware::csrf::csrf_token_from_jar`].
    pub csrf_token: String,
}

/// Redirect response for unauthorized page access
pub struct LoginRedirect;

impl IntoResponse for LoginRedirect {
    fn into_response(self) -> Response {
        FlashRedirect::warning("/login", "Please log in to continue.").into_response()
    }
}

impl FromRequestParts<AppState> for PageAuthUser {
    type Rejection = LoginRedirect;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_request_parts(parts, state)
            .await
            .map_err(|_e| LoginRedirect)?;

        let token = session_token_from_jar(&jar, &state.config.secret).ok_or(LoginRedirect)?;

        let Ok(Some(mut session)) = session::find_by_token(&state.db, &token).await else {
            return Err(LoginRedirect);
        };
        if session.is_expired() {
            let _ = session::delete_session(&state.db, &token).await;
            audit::session_destroyed(&state.config.secret, &token, session.user_id, "expired");
            return Err(LoginRedirect);
        }
        if let Ok(Some(new_expires_at)) = session::refresh_if_needed(&state.db, &session).await {
            audit::session_renewed(
                &state.config.secret,
                &token,
                session.user_id,
                new_expires_at,
            );
            session.expires_at = new_expires_at;
            RotationSlot::request(parts);
        }
        let _ = session::touch_last_seen(&state.db, &session).await;
        let Ok(Some(user)) = user::find_by_id(&state.db, session.user_id).await else {
            return Err(LoginRedirect);
        };
        if user.is_disabled() {
            return Err(LoginRedirect);
        }

        let peer_ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        let via_forward_auth = crate::middleware::forward_auth::forward_auth_identity(
            &state.config,
            peer_ip,
            &parts.headers,
        )
        .is_some();

        Ok(PageAuthUser {
            user,
            session,
            via_forward_auth,
            csrf_token: crate::secret::derive_csrf(&state.config.secret, &token),
        })
    }
}

impl OptionalFromRequestParts<AppState> for PageAuthUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Option<Self>, Self::Rejection> {
        match <PageAuthUser as FromRequestParts<AppState>>::from_request_parts(parts, state).await {
            Ok(user) => Ok(Some(user)),
            Err(_) => Ok(None),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AdminUser {
    pub user: User,
    pub session: Session,
    /// Whether the trusted forward-auth proxy asserted this request's
    /// identity. Carried through from [`AuthUser`] so the handlers that ask
    /// for a recent password confirmation can exempt these sessions the same
    /// way [`RecentlyAuthenticated`] does — the account may hold no usable
    /// password at all, so demanding one would lock the admin out of the very
    /// controls this is meant to protect.
    pub via_forward_auth: bool,
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth_user = AuthUser::from_request_parts(parts, state).await?;

        if auth_user.session.is_masquerading() {
            if let Some(original_user_id) = auth_user.session.original_user_id {
                let original_user = user::find_by_id(&state.db, original_user_id)
                    .await?
                    .ok_or(AppError::Unauthorized)?;
                if !original_user.is_admin() {
                    return Err(AppError::Forbidden);
                }
            } else {
                return Err(AppError::Forbidden);
            }
        } else if !auth_user.user.is_admin() {
            return Err(AppError::Forbidden);
        }

        Ok(AdminUser {
            user: auth_user.user,
            session: auth_user.session,
            via_forward_auth: auth_user.via_forward_auth,
        })
    }
}

/// Admin extractor for page routes that redirects to login on unauthorized
#[derive(Debug, Clone)]
pub struct PageAdminUser {
    pub user: User,
    pub session: Session,
    pub via_forward_auth: bool,
    /// See [`PageAuthUser::csrf_token`].
    pub csrf_token: String,
}

impl FromRequestParts<AppState> for PageAdminUser {
    type Rejection = LoginRedirect;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let page_auth_user =
            <PageAuthUser as FromRequestParts<AppState>>::from_request_parts(parts, state).await?;

        if page_auth_user.session.is_masquerading() {
            if let Some(original_user_id) = page_auth_user.session.original_user_id {
                let Ok(Some(original_user)) = user::find_by_id(&state.db, original_user_id).await
                else {
                    return Err(LoginRedirect);
                };
                if !original_user.is_admin() {
                    return Err(LoginRedirect);
                }
            } else {
                return Err(LoginRedirect);
            }
        } else if !page_auth_user.user.is_admin() {
            return Err(LoginRedirect);
        }

        Ok(PageAdminUser {
            user: page_auth_user.user,
            session: page_auth_user.session,
            via_forward_auth: page_auth_user.via_forward_auth,
            csrf_token: page_auth_user.csrf_token,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Response as HttpResponse;

    fn resp_with_set_cookies(cookies: &[&str]) -> Response {
        let mut builder = HttpResponse::builder();
        for c in cookies {
            builder = builder.header(header::SET_COOKIE, *c);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn response_has_set_cookie_for_matches_exact_name() {
        let resp = resp_with_set_cookies(&["session_token=abc123; Path=/; HttpOnly"]);
        assert!(response_has_set_cookie_for(&resp, "session_token"));
        assert!(!response_has_set_cookie_for(&resp, "csrf_token"));
    }

    #[test]
    fn response_has_set_cookie_for_is_not_fooled_by_value_containing_name() {
        // A different cookie whose *value* happens to contain the target
        // name must not count as a match.
        let resp = resp_with_set_cookies(&["other=session_token_lookalike; Path=/"]);
        assert!(!response_has_set_cookie_for(&resp, "session_token"));
    }

    #[test]
    fn response_has_set_cookie_for_handles_attribute_laden_value() {
        let resp = resp_with_set_cookies(&[
            "csrf_token=xyz; Path=/; SameSite=Lax; Max-Age=604800; Secure",
        ]);
        assert!(response_has_set_cookie_for(&resp, "csrf_token"));
        assert!(!response_has_set_cookie_for(&resp, "session_token"));
    }

    #[test]
    fn response_has_set_cookie_for_checks_each_header_independently() {
        // `anonymous_session` can emit only the CSRF cookie while the session
        // cookie was already present; the session-cookie check must not be
        // fooled by the CSRF header being present, or vice versa.
        let resp = resp_with_set_cookies(&["csrf_token=abc; Path=/"]);
        assert!(response_has_set_cookie_for(&resp, "csrf_token"));
        assert!(!response_has_set_cookie_for(&resp, "session_token"));

        let resp = resp_with_set_cookies(&["session_token=; Path=/", "csrf_token=abc; Path=/"]);
        assert!(response_has_set_cookie_for(&resp, "session_token"));
        assert!(response_has_set_cookie_for(&resp, "csrf_token"));
    }

    #[test]
    fn response_has_set_cookie_for_any_matches_either_name() {
        // The exact scenario slide_session_cookie relies on: logout's removal
        // cookie under the __Host- name must count as "already covered" even
        // though it isn't the unprefixed name.
        let resp = resp_with_set_cookies(&["__Host-session_token=; Path=/; Secure"]);
        assert!(response_has_set_cookie_for_any(
            &resp,
            &[SESSION_COOKIE_NAME, SESSION_COOKIE_NAME_HOST]
        ));
        assert!(!response_has_set_cookie_for_any(
            &resp,
            &[crate::middleware::CSRF_COOKIE_NAME, "__Host-csrf_token"]
        ));
    }

    fn resp_with_cache_control(value: &str) -> Response {
        HttpResponse::builder()
            .header(header::CACHE_CONTROL, value)
            .body(Body::empty())
            .unwrap()
    }

    #[test]
    fn response_is_publicly_cacheable_false_when_no_cache_control_header() {
        let resp = HttpResponse::builder().body(Body::empty()).unwrap();
        assert!(!response_is_publicly_cacheable(&resp));
    }

    #[test]
    fn response_is_publicly_cacheable_false_for_no_store() {
        assert!(!response_is_publicly_cacheable(&resp_with_cache_control(
            "no-store"
        )));
    }

    #[test]
    fn response_is_publicly_cacheable_false_for_private() {
        assert!(!response_is_publicly_cacheable(&resp_with_cache_control(
            "private, max-age=0"
        )));
    }

    #[test]
    fn response_is_publicly_cacheable_true_for_public_max_age() {
        // The proxy's and feed icon's actual directive.
        assert!(response_is_publicly_cacheable(&resp_with_cache_control(
            "public, max-age=86400"
        )));
    }

    #[test]
    fn response_is_publicly_cacheable_true_when_neither_no_store_nor_private() {
        // No `public` either — the rule only excludes no-store/private, it
        // does not require an explicit `public`.
        assert!(response_is_publicly_cacheable(&resp_with_cache_control(
            "max-age=600"
        )));
    }

    #[test]
    fn response_is_publicly_cacheable_directive_match_is_case_insensitive() {
        assert!(!response_is_publicly_cacheable(&resp_with_cache_control(
            "No-Store"
        )));
    }

    #[test]
    fn session_cookie_name_is_prefixed_only_when_secure() {
        assert_eq!(session_cookie_name(true), SESSION_COOKIE_NAME_HOST);
        assert_eq!(session_cookie_name(false), SESSION_COOKIE_NAME);
    }

    #[test]
    fn build_session_cookie_prefixed_variant_carries_secure_and_root_path() {
        let cookie = build_session_cookie("tok", b"01234567890123456789012345678901", true);
        assert_eq!(cookie.name(), SESSION_COOKIE_NAME_HOST);
        assert_eq!(cookie.secure(), Some(true));
        assert_eq!(cookie.path(), Some("/"));
        // A __Host- cookie must never carry a Domain attribute — the browser
        // rejects it outright if it does.
        assert_eq!(cookie.domain(), None);
    }

    #[test]
    fn build_session_cookie_unprefixed_variant_when_not_secure() {
        let cookie = build_session_cookie("tok", b"01234567890123456789012345678901", false);
        assert_eq!(cookie.name(), SESSION_COOKIE_NAME);
        assert_eq!(cookie.secure(), Some(false));
        assert_eq!(cookie.domain(), None);
    }
}
