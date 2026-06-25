# Forward-Auth Logout Fix — Design

Date: 2026-06-25
Status: Approved for planning

## Problem

Under forward-auth, clicking **Sign Out** strands the user: they land on the
login form and cannot get back in — refreshing or visiting `/` keeps redirecting
to `/login`. Manually deleting the `session_token` cookie immediately restores
forward-auth login, which confirmed the root cause.

### Root cause (confirmed)

Two bugs combine into a hard lockout:

1. **Bug 1 — logout never clears the cookie.** `logout` (`src/handlers/auth.rs`)
   calls `jar.remove(SESSION_COOKIE_NAME)` with no path, but the cookie is set
   with `Path=/` (login handler and the forward-auth middleware both use
   `.path("/")`). A removal cookie must match the original path, so the browser
   keeps the now-invalid `session_token`. The codebase's own `flash.rs:154`
   shows the correct pattern (`Cookie::build((NAME, "")).path("/").build()`).
2. **Bug 2 — forward-auth bails on cookie *presence*.** The middleware
   short-circuits on `jar.get(SESSION_COOKIE_NAME).is_some()`, not on session
   *validity*. So any stale/expired cookie permanently prevents forward-auth from
   re-authenticating. (This also reproduces naturally on session expiry, not just
   logout.)

The server-side session *is* deleted on logout, so `PageAuthUser` rejects the
stale cookie and redirects to `/login`; under `DISABLE_LOCAL_AUTH` the form is
hidden, so there is no escape.

### Why the reference apps don't have this

- **Miniflux**: logout clears the server-side session; its auth-proxy middleware
  re-auths on auth *state* (`IsAuthenticated`), not cookie presence.
- **linkding**: Django's `LogoutView` clears the cookie correctly; its
  `RemoteUserMiddleware` re-auths from the *header* on every request. linkding
  also exposes `LD_AUTH_PROXY_LOGOUT_URL` — exactly the option below.

Both converge on **Option A**: local logout clears the session, and the proxy
header re-authenticates on the next request (you bounce back into the app),
while an optional IdP-logout URL provides a real sign-out.

## Goal

Match the linkding/Miniflux behavior (Option A) and add the IdP-logout option:

1. Fix Bug 1 — logout clears the cookie with the matching path.
2. Fix Bug 2 — forward-auth engages when the session cookie is missing **or
   invalid**, not merely absent.
3. `/login` redirects an already-authenticated user to `/` (like Django's login
   view), so post-re-login lands in the app, not on the form.
4. Add `AUTH_PROXY_LOGOUT_URL` — when set, Sign Out redirects there (ends the
   IdP/SSO session); when unset, Sign Out redirects to `/login` (Option A:
   forward-auth bounces the user back in).

## Non-goals

- No "logged-out landing page" that suppresses auto-login (the rejected Option B;
  neither reference app does it).
- No change to GReader/passkey auth.
- No schema change.

## Design

### Item 1 — logout clears the cookie (`src/handlers/auth.rs`)

Follow the `flash.rs` pattern: build a removal cookie with `Path=/`.

```rust
let removal = Cookie::build((SESSION_COOKIE_NAME, "")).path("/").build();
Ok(jar.remove(removal))
```

(Combined with Item 4, the handler returns this jar plus a JSON redirect target.)

### Item 2 — forward-auth checks session validity (`src/middleware/forward_auth.rs`)

Replace the presence-only short-circuit. When a `session_token` cookie is
present, look it up on the **read** connection; pass through only if it is a
valid (non-expired) session. Otherwise continue into the forward-auth logic so a
stale cookie is overwritten with a fresh session.

```rust
// Already carrying a VALID session → leave it to the normal flow.
if let Some(token) = jar.get(SESSION_COOKIE_NAME).map(|c| c.value().to_string()) {
    let valid = state
        .db
        .read_user(move |conn| {
            Ok::<bool, AppError>(
                session::find_by_token(conn, &token)?
                    .map(|s| !s.is_expired())
                    .unwrap_or(false),
            )
        })
        .await
        .map(|r| r.unwrap_or(false))
        .unwrap_or(false);
    if valid {
        return next.run(req).await;
    }
}
```

The `!config.auth_proxy_enabled()` early return stays ahead of this so the check
only runs when forward-auth is on.

### Item 3 — `/login` redirects authenticated users (`src/handlers/pages/mod.rs`)

`login_page` gains an `Option<PageAuthUser>` extractor; when present, redirect to
`/`. Return type becomes `Response`.

```rust
pub async fn login_page(
    State(state): State<AppState>,
    auth: Option<PageAuthUser>,
    flash: Flash,
) -> Response {
    if auth.is_some() {
        return Redirect::to("/").into_response();
    }
    // ... existing LoginTemplate render, returned via .into_response()
}
```

### Item 4 — `AUTH_PROXY_LOGOUT_URL` (`config.rs`, `auth.rs`, sidebar JS)

- **Config:** new field `auth_proxy_logout_url: Option<String>` (env
  `AUTH_PROXY_LOGOUT_URL`, default `None`). No new startup-validation rule
  (harmless if set without forward-auth). Update all four `Config` struct
  literals (config.rs, webauthn.rs, tests/common, tests/statistics).
- **Logout handler:** return the post-logout redirect target so the
  fetch-based Sign Out can navigate to it.

```rust
#[derive(Serialize)]
pub struct LogoutResponse { pub redirect_to: String }

pub async fn logout(...) -> AppResult<(CookieJar, Json<LogoutResponse>)> {
    // delete_session(...) as today
    let removal = Cookie::build((SESSION_COOKIE_NAME, "")).path("/").build();
    let redirect_to = state
        .config
        .auth_proxy_logout_url
        .clone()
        .unwrap_or_else(|| "/login".to_string());
    Ok((jar.remove(removal), Json(LogoutResponse { redirect_to })))
}
```

- **Sidebar JS** (`static/js/components/rdrs-sidebar.js`): use the returned
  target. Preserve the existing "logged out" flash for local paths; navigate
  directly for an external IdP URL.

```js
const r = await fetch('/api/session', { method: 'DELETE' });
if (r.ok) {
    const d = await r.json();
    if (d.redirect_to.startsWith('/')) {
        window.flash.redirect(d.redirect_to, 'info', 'You have been logged out.');
    } else {
        window.location.href = d.redirect_to;
    }
} else {
    window.flash.error('Logout failed');
}
```

## Resulting behavior

| Deployment | Sign Out result |
|---|---|
| Password only (no forward-auth) | Cookie cleared, land on `/login`, log in again. |
| Forward-auth, no `AUTH_PROXY_LOGOUT_URL` | Cookie cleared → `/login` → forward-auth re-auths from the still-valid IdP session → `/login` redirects to `/` → **back in the app** (Option A, matches linkding/Miniflux). |
| Forward-auth + `AUTH_PROXY_LOGOUT_URL` | Cookie cleared → browser navigates to the IdP logout URL → **SSO session ends** → returning to rdrs requires a fresh IdP login. |

## Testing

- **Item 1:** logout response sets `session_token` deletion cookie with `Path=/`
  (assert the `Set-Cookie` header path); after login+logout the test client no
  longer holds a valid session.
- **Item 2:** integration — with a present-but-invalid `session_token` cookie and
  a trusted forward-auth header, the middleware still establishes a fresh
  session (does not bounce to `/login`). Also: a present **valid** session cookie
  is left untouched (no new session minted).
- **Item 3:** `GET /login` with a valid session redirects to `/` (302); without a
  session renders the form.
- **Item 4:** logout `redirect_to` is `/login` by default and equals
  `AUTH_PROXY_LOGOUT_URL` when configured; config parsing of the new var.
- Existing `test_logout` updated for the new JSON response shape.
- Check `e2e/` logout steps still pass (default redirect is `/login`).

## Affected files

- `src/config.rs` — `auth_proxy_logout_url` + parsing; 4 literals updated.
- `src/handlers/auth.rs` — cookie clearing + `LogoutResponse`.
- `src/middleware/forward_auth.rs` — validity check.
- `src/handlers/pages/mod.rs` — `/login` authed redirect.
- `static/js/components/rdrs-sidebar.js` — Sign Out redirect handling.
- `ARCHITECTURE.md`, `README.md` — document `AUTH_PROXY_LOGOUT_URL` and the
  logout behavior.
- Tests: `tests/auth_test.rs`, `tests/forward_auth_test.rs`, the 4 Config
  literals.

## Out-of-scope follow-ups

- Recording the auth method on the session (to tailor Sign Out per-method).
- RP-initiated OIDC logout (not applicable to forward-auth).
