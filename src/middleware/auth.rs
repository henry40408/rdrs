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

/// `__Host-`-prefixed session cookie name, used only when `Secure` is in effect
/// (see [`session_cookie_name`]).
///
/// OWASP's Session Management Cheat Sheet recommends the prefix wherever
/// possible: a browser enforces that a cookie under this name is `Secure`,
/// carries `Path=/`, has no `Domain`, and was set by this exact host — closing
/// the channel by which a sibling subdomain could otherwise *write* a cookie
/// that shadows ours.
///
/// Defence in depth, not a fix for a real gap: every cookie value carries an
/// HMAC signature that [`session_token_from_jar`] verifies before any database
/// lookup, so a malicious subdomain cannot mint a value that verifies, prefix or
/// no prefix.
///
/// A `__Host-` cookie without `Secure` is silently *discarded* by the browser,
/// so this name must never be written while `Secure` is off — that would make
/// login impossible on a plain-HTTP deployment, rdrs's own default. Hence it is
/// only ever selected via [`session_cookie_name`].
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
/// Every login path goes through here so the attributes cannot drift apart. The
/// cookie *value* is the token plus an HMAC signature, so a forged cookie is
/// rejected before any database lookup and a leaked `session.session_token` is
/// unusable without the root key. `secure` comes from
/// [`crate::config::Config::cookie_secure`] and also picks the cookie *name* via
/// [`session_cookie_name`]; `Path=/` and no `Domain` are already true below, so
/// whenever `secure` is on this cookie is fully prefix-compliant.
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
/// verify. Every extractor and the forward-auth middleware funnel through here.
///
/// Tries [`SESSION_COOKIE_NAME_HOST`] first, then falls back to the unprefixed
/// [`SESSION_COOKIE_NAME`] — not only when the prefixed cookie is absent, but
/// also when it is present and fails to verify. Accepting the unprefixed name
/// weakens nothing (forgery resistance comes entirely from the HMAC), and it is
/// necessary: an upgrade, or an operator flipping `RDRS_COOKIE_SECURE`, must not
/// log out every existing session. Falling through past a present-but-invalid
/// prefixed cookie matters too — a browser can carry a stale, empty `__Host-`
/// cookie from a logout's removal `Set-Cookie` alongside a valid unprefixed one,
/// and that must not shadow it.
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

/// The channel by which an extractor asks [`slide_session_cookie`] to rotate the
/// session token.
///
/// The decision is made deep inside the request, where the sliding refresh
/// already computes "this session is due", but the rotation itself must not
/// happen there: an extractor sees only request parts, so it cannot know whether
/// the response is one a cookie may ride on, and a rotation whose new token
/// never reaches the client would sign that client out once the grace interval
/// lapsed. So the extractor raises the flag and the middleware rotates once it
/// holds the finished response and has ruled out the publicly cacheable ones.
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

/// Path prefixes skipped entirely: static assets, favicons and the health check
/// must stay cacheable, and a `Set-Cookie` on any of them would poison a shared
/// cache. Deliberately narrower than `ANON_SKIP_PREFIXES` in `csrf.rs` — `/api`
/// and `/reader` are *not* skipped, since a pure-API client must still get its
/// cookie's `Max-Age` renewed.
///
/// Not the whole defence: the image proxy and the feed icon are deliberately
/// publicly cacheable and are not listed here, since `/api` at large must still
/// be slid. Those are caught on the way out — see
/// [`response_is_publicly_cacheable`].
const SLIDE_SKIP_PREFIXES: &[&str] = &["/static", "/favicon", "/health", "/sw.js", "/offline"];

/// Reissue the session and CSRF cookies on every authenticated request so their
/// `Max-Age` — aligned with the sliding session TTL rather than the old 90-day
/// absolute cap — tracks "last used" the way the row's own `expires_at` does,
/// instead of logging out a browser still in active use.
///
/// Sliding the row's `expires_at` remains `session::refresh_if_needed`'s job.
/// The one database write this layer performs is the token rotation the
/// extractors ask for (see [`RotationSlot`]), which has to happen here because
/// only here is it known whether a new cookie can be delivered at all. An absent
/// or HMAC-invalid session cookie is passed through untouched. A verified but
/// row-less *anonymous* cookie is slid the same way, which is harmless and
/// intentional: it expires on its own schedule and has no row to out-live.
///
/// Layered outside `anonymous_session` and inside `forward_auth`, so it observes
/// — and must never clobber — the `Set-Cookie`s those emit. For each cookie
/// *purpose* (session, CSRF), either of whose two names may be in play, a
/// `Set-Cookie` already present under *either* name is left alone; only the
/// absence of both appends a fresh one. Checking both names matters most for
/// `logout`, which emits removal cookies under all four: recognising only the
/// name this layer would itself write would append a *live* cookie next to a
/// removal and silently undo the logout.
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

    // A live session cookie must never ride on a response a shared cache may
    // store — that cache would hand this user's session to the next visitor.
    // `proxy_image` and `get_feed_icon` deliberately mark their responses
    // `public, max-age=...`, so skip the reissue for those rather than trying to
    // enumerate their paths.
    //
    // Checked *before* the rotation below: a rotation this response cannot carry
    // would rename the session behind the client's back and sign it out once the
    // grace interval lapsed.
    if response_is_publicly_cacheable(&resp) {
        return resp;
    }

    // The rotation happens here rather than in the extractor that asked for it,
    // because only here is it known the new token can reach the client.
    // `rotate_token` matches on the old token, so if a concurrent request beat us
    // we get `None` and keep the token we have — still valid, because the winner
    // left it behind as the grace token.
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
/// Every `Set-Cookie` value is split at its *first* `=` and the trimmed
/// substring before it compared for an exact match. Deliberately not a substring
/// search: a different cookie whose value happens to contain `name` must not
/// count.
fn response_has_set_cookie_for(resp: &Response, name: &str) -> bool {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| v.split_once('=').is_some_and(|(n, _)| n.trim() == name))
}

/// Whether `resp` already carries a `Set-Cookie` for *any* of `names` — i.e.
/// whether this cookie purpose is already covered, where the purpose may be
/// represented by either its unprefixed or `__Host-`-prefixed name. See
/// [`slide_session_cookie`].
fn response_has_set_cookie_for_any(resp: &Response, names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| response_has_set_cookie_for(resp, name))
}

/// Whether `resp` declares itself storable by a *shared* cache, in which case a
/// `Set-Cookie` must not be stapled to it. `no_store_for_authenticated` is
/// layered inside this middleware and has already run, so an ordinary
/// authenticated response carries `no-store` from it, while the deliberate
/// public-caching call sites carry their own `public, max-age=...` untouched.
///
/// "Shared-cacheable" here means a `Cache-Control` whose directives include
/// neither `no-store` nor `private`; an absent or non-UTF-8 header is treated as
/// not cacheable, so the default is to still slide the cookie.
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
/// Events", applied to the operations that change which credentials can open the
/// account.
///
/// It guards passkey registration and removal. Registering a passkey adds an
/// independently usable credential a later password change will *not* revoke, so
/// a session someone else picked up — an unlocked laptop, a borrowed browser —
/// must not be able to mint one silently.
///
/// Forward-auth sessions are exempt, and the exemption is not a gap: their
/// identity is asserted by the proxy on every request, so rdrs has no credential
/// of its own to re-check, and the account may hold no usable password at all.
/// Requiring one there would lock those users out of passkey registration
/// permanently rather than protect them.
///
/// Rejects with [`AppError::ReauthenticationRequired`], which the browser turns
/// into a password prompt and a retry.
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
    /// re-derives from the cookie the browser sends back, and during a rotation's
    /// grace interval those two differ.
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
    /// Whether the trusted forward-auth proxy asserted this request's identity.
    /// Carried through from [`AuthUser`] so handlers asking for a recent password
    /// confirmation can exempt these sessions the way [`RecentlyAuthenticated`]
    /// does — the account may hold no usable password, so demanding one would
    /// lock the admin out of the very controls this protects.
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
