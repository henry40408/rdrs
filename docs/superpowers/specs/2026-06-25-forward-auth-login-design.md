# Forward-Auth (Trusted-Header) Login — Design

Date: 2026-06-25
Status: Approved for planning

## Goal

Let users sign in to rdrs through an upstream identity provider (Authelia and
friends) that performs **forward-auth**: the reverse proxy authenticates the
request and injects an identity header, which rdrs trusts to establish a local
session. Existing password/passkey accounts must keep working, and existing
accounts must migrate with zero data loss.

## Why forward-auth (not OIDC)

Forward-auth / trusted-header SSO is the mainstream way to put SSO in front of
apps that do not natively speak OIDC. The protocol complexity (discovery, code
flow, JWT validation) stays in the proxy + auth service; rdrs only has to read
one header safely. By making the header name configurable, a single
implementation is compatible with **all** common providers — Authelia
(`Remote-User`), authentik (`X-authentik-username`), oauth2-proxy
(`X-Forwarded-User`), Vouch, Pomerium, Cloudflare Access, Tailscale, etc. —
not just Authelia.

OIDC was considered and rejected for this iteration: for the browser case it
solves the same problem as forward-auth but adds a relying-party implementation
in rdrs. It may be revisited later as an independent feature.

## Non-goals

- No OIDC relying-party support.
- No `external_id` / email-based account linking. Mapping is by **username**.
- No schema change / database migration.
- No change to GReader API or passkey authentication mechanics.

## Account mapping & migration

- The identity header carries a **username**. rdrs looks it up with
  `user::find_by_username`. This is the entire migration story: existing
  password accounts whose username equals the Authelia username log straight in
  and keep all their feeds / read state. **No schema change, no migration.**
- Precondition: the upstream username must equal the existing rdrs username.
  Mismatches are resolved by an admin renaming the rdrs account to align. An
  `external_id` mapping column is an explicit out-of-scope follow-up.

## Configuration (new env vars in `config.rs`)

| Variable | Default | Purpose |
|---|---|---|
| `AUTH_PROXY_HEADER` | `""` (disabled) | Header carrying the username, e.g. `Remote-User`. Empty disables the whole feature. |
| `TRUSTED_PROXY_NETWORKS` | `""` | Comma-separated CIDR list; the TCP peer IP must fall inside one of these for the header to be trusted. |
| `AUTH_PROXY_USER_CREATION` | `false` | When the username is unknown, JIT-create a local account instead of rejecting. |
| `DISABLE_LOCAL_AUTH` | `false` | Hide the browser password login form and reject `POST /api/session`. Does **not** affect GReader API auth or passkeys. |
| `AUTH_PROXY_GROUPS_HEADER` | `""` | Header carrying comma-separated groups, e.g. `Remote-Groups`. |
| `AUTH_PROXY_ADMIN_GROUP` | `""` | Membership of this group grants the `Admin` role. |

Group → role mapping is **active only when both** `AUTH_PROXY_GROUPS_HEADER`
and `AUTH_PROXY_ADMIN_GROUP` are set.

### Startup validation (mirrors Miniflux)

- `AUTH_PROXY_HEADER` set but `TRUSTED_PROXY_NETWORKS` empty → refuse to start
  (header trust without a trusted-source check is a spoofing hole).
- `DISABLE_LOCAL_AUTH` set but `AUTH_PROXY_HEADER` empty → refuse to start (no
  alternative browser login would remain).
- Invalid CIDR entries in `TRUSTED_PROXY_NETWORKS` → refuse to start.

## Authentication flow (new tower middleware)

A middleware applied to the SSR page routes and `/login`. It runs only when the
request has **no valid session cookie**:

1. Feature disabled (`AUTH_PROXY_HEADER` empty) → pass through unchanged.
2. Read the **TCP peer IP** from `ConnectInfo<SocketAddr>` (never
   `X-Forwarded-For`). If it is not inside `TRUSTED_PROXY_NETWORKS` → pass
   through (treat as if no forward-auth).
3. Read the username from `AUTH_PROXY_HEADER`. Empty → pass through.
4. Resolve the account:
   - **Found, not disabled** → derive role (see below), create a session, set
     the session cookie, and redirect to the originally requested URL. (The
     cookie takes effect on the redirected request — same approach as Miniflux,
     one extra round-trip on first login only.)
   - **Found, disabled** → reject.
   - **Not found + `AUTH_PROXY_USER_CREATION`** → JIT-create the account
     (unusable sentinel password hash `"!"`, seed an `Uncategorized` category as
     registration does), assign role, then create the session.
   - **Not found + creation disabled** → reject (redirect to `/login` with a
     warning flash).

"Reject" = redirect to `/login` with a flash; it does not 500.

### Why the sentinel password hash is safe

`verify_password` returns `false` for any unparseable hash (confirmed in
`auth/password.rs`: `PasswordHash::new` fails → `false`). Storing `"!"` makes
local password login impossible for JIT accounts without a special case.

## Role handling

- **No group mapping configured:** roles are purely local. JIT-created accounts
  follow the existing bootstrap rule — `Admin` when the user count is 0,
  otherwise `User`. Admins are promoted/demoted manually in the existing admin
  UI.
- **Group mapping configured:** on **every** forward-auth login the role is
  recomputed from the groups header — `Admin` if `AUTH_PROXY_ADMIN_GROUP` is in
  the parsed group list, else `User` — and persisted if it changed. Authelia
  becomes the authoritative source of role for forward-auth logins. This
  overrides the bootstrap rule for JIT creation too.
- Group mapping only affects forward-auth logins; a local-password login carries
  no groups header and never changes the stored role.
- **Documented caveat:** with group mapping on, a manually-set local admin can
  be demoted on next forward-auth login, and a misconfigured admin group can
  leave the instance with no admin. This is the operator's responsibility.

## Coexistence with local auth & API clients

- forward-auth, local password, and passkey coexist by default.
- `DISABLE_LOCAL_AUTH=true` hides the `/login` password form and rejects
  `POST /api/session`, **but** GReader `ClientLogin` and passkey API endpoints
  keep working — native clients (FeedMe, Read You) and security keys must not be
  locked out.
- Deployment note (docs only): the proxy must be configured to bypass
  forward-auth for API paths (`/accounts/ClientLogin`, `/reader/api/...`) so
  native clients can still reach them.

## Security model

- `main.rs` must switch from `axum::serve(listener, app)` to
  `axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())`
  so the middleware can read the genuine TCP peer IP. (Confirmed: the project
  does not currently wire `ConnectInfo`.)
- The identity/groups headers are read **only** after the peer-IP trust check
  passes, preventing a direct client from spoofing `Remote-User: admin`.
- CIDR matching uses the `ipnet` crate (to be added, pinned to a version at
  least 7 days old per the dependency cooldown policy).
- Docs must warn operators that the proxy has to **authoritatively overwrite or
  strip** the inbound identity/groups headers, so a downstream client cannot
  smuggle them through.

## Affected code

- `src/config.rs` — new fields, parsing, startup validation, tests.
- `src/main.rs` — wire `ConnectInfo<SocketAddr>`.
- `src/middleware/` — new `forward_auth.rs` middleware + wiring in `lib.rs`.
- `src/middleware/auth.rs` — unchanged extractors; the new middleware runs
  before them and only acts when no session cookie is present.
- `src/models/user.rs` — reuse `find_by_username`, `create_user`,
  `update_role`; no schema change.
- `templates/` — `/login` conditionally hides the password form under
  `DISABLE_LOCAL_AUTH`.
- `Cargo.toml` — add `ipnet`.

## Testing

- **Unit:** config parsing + startup validation (header requires trusted
  networks; disable-local-auth requires header; invalid CIDR rejected); CIDR
  membership; group parsing → role; sentinel hash never verifies.
- **Integration:**
  - trusted IP + existing user → session created, access granted;
  - unknown user + creation disabled → rejected;
  - unknown user + creation enabled → user created + session;
  - header from untrusted IP → header ignored, no auto-login;
  - disabled user → rejected;
  - group mapping: admin group present → Admin; absent → demoted to User on next
    login;
  - `DISABLE_LOCAL_AUTH` blocks `POST /api/session` but GReader `ClientLogin`
    still succeeds.

## Docs

- Update `ARCHITECTURE.md` (authentication section) and the `README` env-var
  table.
- No screenshot regeneration: default config leaves `/login` visually
  unchanged; the password form only disappears when `DISABLE_LOCAL_AUTH` is
  explicitly enabled (non-default).

## Out-of-scope follow-ups

- OIDC relying-party support.
- `external_id` column for username-independent account linking.
- email-based mapping (needs a new `email` column + backfill).
- Per-group fine-grained permissions beyond admin/user.
