# Auth-Mode Indicator + App Config Grouping — Design

Date: 2026-06-25
Status: Approved for planning

## Goal

Make it obvious how a user is authenticated and what auth-related configuration
the instance runs:

1. **Sidebar SSO pill** — when the current session was established via
   forward-auth, show a small "SSO" pill hugging the username in the sidebar
   footer. Local (password/passkey) sessions show no pill.
2. **App page forward-auth config** — the `/settings` ("App") Configuration
   table lists the forward-auth env vars, and the whole table is reorganized
   into labeled groups with the columns reordered to **Variable · Current ·
   Default · Description**.

## Why a schema change is needed

The pill must reflect *how the current session authenticated*. The `session`
row does not record that today, and it cannot be inferred after the fact. So we
persist the auth method on the session at creation time.

## Components

### 1. `session.auth_method` (schema migration)

- Add column `auth_method TEXT NOT NULL DEFAULT 'local'` to `session` via a new
  `PRAGMA user_version` migration in `db/schema.rs`.
- `Session` struct gains `pub auth_method: String`; `row_to_session` reads it;
  every `SELECT` of session columns (`find_by_token`, `find_by_id`, any others)
  includes it.
- `create_session(conn, user_id)` becomes
  `create_session(conn, user_id, auth_method: &str)`; the `INSERT` writes the
  column. Update all four call sites:
  - `src/middleware/forward_auth.rs` → `"forward_auth"`
  - `src/handlers/auth.rs` (password login) → `"local"`
  - `src/handlers/passkey.rs` (passkey) → `"local"`
  - `src/handlers/greader/auth.rs` (ClientLogin) → `"local"`
- Values are limited to `"local"` and `"forward_auth"` (distinguishing
  password vs passkey is out of scope — the UI only needs forward-auth vs not).
  Masquerade does not call `create_session` (it mutates an existing session), so
  it is unaffected.

### 2. `SidebarResponse.via_forward_auth` (backend exposure)

- Add `pub via_forward_auth: bool` to `crate::handlers::user::SidebarResponse`
  (used by BOTH the SSR sidebar bootstrap and the `GET /api/sidebar` endpoint).
- `build_app_layout` (`handlers/pages/mod.rs`) sets it from the current session:
  `via_forward_auth: auth_user.session.auth_method == "forward_auth"`.
- `get_sidebar` (`handlers/user.rs`, the `/api/sidebar` handler) sets the same
  field from its `AuthUser`'s session, so a live refresh keeps the pill correct.
- `serialize_sidebar_for_script` serializes the struct as-is, so the bootstrap
  JSON gains the field automatically.

### 3. Sidebar SSO pill (frontend)

- In `static/js/components/rdrs-sidebar.js`, read `via_forward_auth` from the
  bootstrap/sidebar data and, when true, render a pill immediately after the
  username in `.sidebar-footer`:

  ```html
  <div class="sidebar-footer">
    <span class="sidebar-id">
      <span class="sidebar-user">alice</span>
      <span class="sidebar-auth-pill">SSO</span>   <!-- only when via_forward_auth -->
    </span>
    <a ... data-rdrs-logout>Sign Out</a>
  </div>
  ```

- The full-rerender trigger (`needsFullRerender`) must treat a change in
  `via_forward_auth` as requiring a rerender (add it alongside the existing
  `username` check) so a live `/api/sidebar` update can add/remove the pill.
- CSS (`static/css/app.css`): `.sidebar-id` is a tight flex row
  (`display:flex; align-items:center; gap:6px; min-width:0`) so the name can
  ellipsize and the pill stays put. `.sidebar-auth-pill` reuses the
  `.sidebar-badge` look — `font-size: var(--font-xs); font-weight: 600;
  color: var(--color-accent); background: var(--color-accent-subtle);
  padding: 0.1em 0.5em; border-radius: var(--radius-lg); flex: none;`. Local
  sessions render no pill (no neutral "Local" pill).

### 4. App page: grouped config table, reordered columns, forward-auth rows

- `SettingsTemplate` (`handlers/pages/mod.rs`) gains the forward-auth config
  fields, read from `state.config`: `auth_proxy_header: String`,
  `trusted_proxy_networks: String` (rendered, e.g. comma-joined), 
  `auth_proxy_user_creation: bool`, `auth_proxy_groups_header: String`,
  `auth_proxy_admin_group: String`, `disable_local_auth: bool`,
  `auth_proxy_logout_url: String`. Empty/unset values render as a muted
  "Not set" / boolean as Yes/No, matching the existing rows' conventions.
- `templates/settings.html` Configuration table is rewritten:
  - **Column order:** `Variable · Current · Default · Description` (Current
    second, next to the name; Description prose last).
  - **Groups** (a `.grouphdr` row — `<tr class="grouphdr"><td colspan="4">…`):
    1. **Server** — `DATABASE_URL`, `SERVER_PORT`
    2. **Accounts & Registration** — `SIGNUP_ENABLED`, `MULTI_USER_ENABLED`
    3. **Authentication — Passkeys (WebAuthn)** — `WEBAUTHN_RP_ID`,
       `WEBAUTHN_RP_ORIGIN`, `WEBAUTHN_RP_NAME`
    4. **Authentication — Forward-Auth** — `AUTH_PROXY_HEADER`,
       `TRUSTED_PROXY_NETWORKS`, `AUTH_PROXY_USER_CREATION`,
       `AUTH_PROXY_GROUPS_HEADER`, `AUTH_PROXY_ADMIN_GROUP`,
       `DISABLE_LOCAL_AUTH`, `AUTH_PROXY_LOGOUT_URL`
    5. **Content & Media** — `USER_AGENT`, `IMAGE_PROXY_SECRET`
  - Keep `data-label` attributes on each `<td>` for the responsive
    `mobile-cards-settings` card view; verify card view still reads sensibly
    after the reorder (the group header `<tr>` must render acceptably or be
    hidden in card view).
- CSS additions (`static/css/app.css`):
  - `.settings-th` (the header cells) get **symmetric vertical padding** so the
    header reads balanced (the current rule that yields `0` top padding is the
    cause of the unbalanced look once a column is tinted).
  - `.grouphdr td` — accent-tinted, uppercase, small label row.
  - The Current column gets a faint tint and `vertical-align: middle` so its
    top/bottom spacing stays even across single- and multi-line rows. (Header
    + body Current cells share a class, e.g. `.settings-col-current`.)

### 5. Documentation

- `ARCHITECTURE.md` forward-auth section: add ~2 sentences — the sidebar shows
  an **SSO** pill when the current session authenticated via forward-auth
  (backed by the new `session.auth_method`), and the App page surfaces the
  forward-auth configuration. The six/seven env vars are already documented; do
  not duplicate them.

## Data flow

login/forward-auth/passkey/ClientLogin → `create_session(..., auth_method)` →
`session.auth_method` persisted → `AuthUser`/`PageAuthUser` carries the session
→ `build_app_layout` / `get_sidebar` compute `via_forward_auth` →
bootstrap JSON / `/api/sidebar` → `rdrs-sidebar.js` renders the pill.

## Testing

- **Schema/model:** migration adds the column with default `'local'`;
  `create_session` persists the passed method; `find_by_token`/`find_by_id`
  round-trip `auth_method`. An existing-DB row (pre-migration) reads back as
  `'local'`.
- **Backend exposure (integration):** a forward-auth login yields a sidebar
  bootstrap / `/api/sidebar` with `via_forward_auth: true`; a password login
  yields `false`.
- **App page (integration):** `GET /settings` contains the four group headers
  and the seven forward-auth variable names, with the configured "Current"
  values reflected (e.g. `AUTH_PROXY_HEADER` shows the set header name).
- **Frontend:** covered by the integration data assertions plus a manual/e2e
  check that the pill appears only under forward-auth.

## Screenshots

The four README screenshots are the unread list (+ reading pane) and the
keyboard-help overlay, captured with demo data under default (local) config —
no forward-auth, so **no SSO pill renders** and the sidebar footer is visually
unchanged. `/settings` is not screenshotted. Therefore no screenshot
regeneration is expected; confirm via `cd e2e && npm run screenshots` showing no
diff, and only commit regenerated images if a diff actually appears.

## Affected files

- `src/db/schema.rs` — migration.
- `src/models/session.rs` — `Session.auth_method`, `create_session` signature,
  row mapping, SELECT/INSERT columns; tests.
- `src/middleware/forward_auth.rs`, `src/handlers/auth.rs`,
  `src/handlers/passkey.rs`, `src/handlers/greader/auth.rs` — pass the method.
- `src/handlers/user.rs` — `SidebarResponse.via_forward_auth` + `get_sidebar`.
- `src/handlers/pages/mod.rs` — `build_app_layout` flag; `SettingsTemplate`
  forward-auth fields + population.
- `static/js/components/rdrs-sidebar.js` — pill rendering + rerender trigger.
- `templates/settings.html` — grouped, reordered table.
- `static/css/app.css` — `.sidebar-id`, `.sidebar-auth-pill`, `.grouphdr`,
  Current-column tint + balanced header padding.
- `ARCHITECTURE.md` — short note.
- Tests: `tests/auth_test.rs` / `tests/forward_auth_test.rs` / `tests/pages_test.rs`
  as appropriate; `src/models/session.rs` unit tests.

## Out-of-scope follow-ups

- Distinguishing password vs passkey in `auth_method` (only `local` vs
  `forward_auth` now).
- A "Sign-in method" row on the User Settings page (rejected option C).
- Per-provider pill label from a configured provider name (label is the static
  "SSO").
