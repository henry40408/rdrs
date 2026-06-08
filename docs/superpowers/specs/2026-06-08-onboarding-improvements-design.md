# Onboarding Improvements — Design

**Issue:** #269 — Improve onboarding: from install to first successful read
**Date:** 2026-06-08
**Status:** Approved

## Problem

The fresh-install happy path (*found the project* → *reading feeds*) is broken by two
hard blockers and degraded by several first-run/deployment gaps:

1. 🔴 **Source-build users are locked out.** `SIGNUP_ENABLED` defaults to `false`, and
   `can_register` requires it even for the very first account, so a source build can never
   create its first user without manual DB surgery.
2. 🔴 **Can't add the first feed.** The Add Feed `<select required>` only offers an empty
   `No categories available` option for a new user; the browser silently blocks submission.
3. 🟠 **Passkey silently fails off-localhost.** `WEBAUTHN_RP_ID`/`RP_ORIGIN` default to
   `localhost`; deploying behind a domain without overriding them breaks passkeys with no
   log or UI signal, and Settings never shows the active values.
4. 🟠 **`IMAGE_PROXY_SECRET` rotates on restart when unset.** The `docker run` quick-start
   (the most-copied snippet) omits it, so proxied image URLs break on every restart.
5. 🟡 **Misleading landing empty state.** `/` (Unread) hardcodes "All caught up" and never
   distinguishes "no feeds yet" from "all read".
6. 🟡 **No getting-started affordance.** No checklist or CTA points a brand-new user at
   adding a feed or importing OPML.

## Scope decisions

- **#5 and #6 are merged** into a single SSR onboarding empty state. They share the same
  trigger (account has zero feeds) and the same screen location, so rendering both would
  duplicate guidance. One block covers both.
- **No dismiss control** for the onboarding block. It is shown only while the account has
  zero feeds and disappears naturally once the first feed exists — no JS, no cookie, no
  persisted state.
- **#1's "actionable hint for zero-user lockout" is dropped as moot.** Once the first
  account can always register, the zero-user lockout cannot occur, so no special hint is
  needed. The generic "Registration is currently disabled" message remains for the
  legitimately-disabled case (≥1 user, signup not enabled).

## Design

### #1 — First account can always register

`Config::can_register` changes so the first account is always allowed, while the flag still
gates every subsequent account:

```rust
pub fn can_register(&self, user_count: i64) -> bool {
    user_count == 0 || (self.signup_enabled && self.multi_user_enabled)
}
```

Behaviour table:

| user_count | signup_enabled | multi_user_enabled | before | after |
|------------|----------------|--------------------|--------|-------|
| 0          | false          | false              | ❌     | ✅    |
| 0          | true           | false              | ✅     | ✅    |
| 1          | true           | false              | ❌     | ❌    |
| ≥1         | true           | true               | ✅     | ✅    |
| ≥1         | false          | *                  | ❌     | ❌    |

The only behavioural change is the first row: the very first account now registers even
when `SIGNUP_ENABLED=false`. The single-user-with-signup and multi-user paths are
unchanged.

**README:** the *Building from Source* section gains a note that the first account always
works and `SIGNUP_ENABLED` only controls additional registrations.

### #2 — Seed "Uncategorized" at registration

The **registration handler** (`handlers::auth::register`) seeds a default category named
`Uncategorized` for the new user, in the same `db.user(...)` closure right after
`create_user`, so a fresh account has a usable category and the Add Feed form's `<select>`
is never empty. This matches the existing `Uncategorized` convention used by OPML import and
the GReader subscription API.

Seeding lives in the handler rather than `models::user::create_user` because `create_user`
is a low-level building block reused by dozens of model/integration test fixtures (and by
nothing else that creates real accounts). Passkeys are added to *already-authenticated*
users via `passkey::finish_registration`, so the password `register` handler is the only
genuine account-creation entry point. Keeping `create_user` pure avoids broad test churn.

The `feeds.html` empty-`<option>` branch becomes unreachable for real users but is left in
place as a defensive fallback.

**Sidebar-cache interaction.** `read_chrome_data` previously skipped caching the sidebar
only when an account had *no categories*. Seeding `Uncategorized` makes that always false,
so a brand-new account would cache a `[Uncategorized]`-only sidebar and mask
direct-DB-seeded feeds (which E2E relies on) behind the 60 s TTL. The cache gate is changed
to skip when the account has *no feeds and no unread* (`has_feeds || total_unread > 0`),
restoring the "don't cache an empty account" invariant. The `category_nav` E2E background
removes the seeded `Uncategorized` so its wrap-around scenarios still exercise a clean
three-category set.

### #3 — Surface and warn about WebAuthn RP config

- **Startup warning:** in `main.rs`, after building `Config`, log a `tracing::warn!` when
  the effective RP origin still points at `localhost` **or** disagrees with a configured
  `PUBLIC_BASE_URL`. The message names the active RP origin and explains passkeys will fail
  off-localhost.
- **Settings page:** add three read-only rows to the config table showing the active
  `WEBAUTHN_RP_ID`, `WEBAUTHN_RP_ORIGIN`, and `WEBAUTHN_RP_NAME`. `SettingsTemplate` gains
  the corresponding fields.
- **README:** document `WEBAUTHN_RP_ID`/`RP_ORIGIN` as required when deploying behind a
  domain (the Configuration table already lists them; add a deployment note).

### #4 — `IMAGE_PROXY_SECRET` in the docker quick-start

The `docker run` quick-start in README gains `-e IMAGE_PROXY_SECRET=...` with a generate
note (`openssl rand -base64 32`) and a one-line call-out that, left unset, the secret
regenerates on restart and invalidates previously-proxied image URLs. The startup warning
already exists in `main.rs` and is unchanged.

### #5 + #6 — Onboarding empty state on the landing page

`unread_page` determines whether the account has any feeds (a new
`feed::count_by_user(conn, user_id)` returning `i64`, or reuse of an existing list length).
The result selects the empty-state content rendered by `_entries_layout.html`:

- **Zero feeds** → onboarding copy: a welcome title, a 3-step list
  (1) add a feed or import OPML, (2) wait for the first sync, (3) read — and two CTA links:
  **Add your first feed →** (`/feeds`) and **Import OPML** (`/feeds/import`).
- **Has feeds, all read** → the existing "All caught up" / "You've read every unread
  entry…" copy, unchanged.

Implementation: `EntriesLayoutContext` gains an optional onboarding flag/structure (e.g.
`onboarding: Option<OnboardingView>`) so `_entries_layout.html` renders the CTA block
instead of the plain title/detail when present. This is scoped to the landing (`/`) handler
only; other empty states keep their current title/detail.

## Components touched

| Area | File | Change |
|------|------|--------|
| Config logic | `src/config.rs` | `can_register` rule; unit tests |
| User creation | `src/models/user.rs` | seed `Uncategorized`; tests |
| Register handler | `src/handlers/auth.rs` | unchanged logic, benefits from #1 |
| Landing page | `src/handlers/pages.rs` | feed-count → onboarding vs all-caught-up |
| Feed model | `src/models/feed.rs` | `count_by_user` |
| Sidebar cache | `src/handlers/user.rs` | gate caching on feeds/unread, not on having any category (see note) |
| Settings page | `src/handlers/pages.rs`, `templates/settings.html` | RP id/origin/name rows |
| Startup | `src/main.rs` | RP-origin warning |
| Templates | `templates/_entries_layout.html` | onboarding empty-state block |
| Docs | `README.md` | source-build note, docker `IMAGE_PROXY_SECRET`, RP deploy note |

## Testing

**BDD (`e2e/features/onboarding.feature`):**

- A freshly-registered user's Add Feed form offers an `Uncategorized` category and a feed
  can be added immediately without first creating a category. *(#2)*
- A signed-in user with no feeds sees the getting-started guide ("Add your first feed" /
  "Import OPML" CTAs) on the landing page. *(#5/#6)*
- An account that has a feed shows "All caught up" and not the getting-started guide. *(#5)*
- The Settings page shows the active WebAuthn RP origin. *(#3, UI portion)*

**Unit / Rust tests:**

- `config::can_register` truth table including the new first-account-always row. *(#1)*
- `create_user` seeds exactly one `Uncategorized` category for the new user. *(#2)*
- `feed::count_by_user` (if added). *(#5)*

**Why #1 is not BDD-covered:** the shared, worker-scoped e2e server fixture is hard-wired to
`SIGNUP_ENABLED=true` / `MULTI_USER_ENABLED=true` and is reused across parallel scenarios, so
"signup disabled, zero users, first account still registers" cannot be expressed without a
dedicated isolated server. The behaviour is config logic, so it is verified by the
`config::can_register` unit test instead. (An isolated-server BDD scenario could be added later
if desired.)

**Not BDD-covered:** the startup RP-origin warning (#3 log) and the README/docker doc
changes (#4) — covered by code review and (for the warning) a focused unit test on the
warning predicate if one is extracted.

## Out of scope

- No registration UI redesign, no signup-flow changes beyond `can_register`.
- No dismiss/persistence mechanism for the onboarding block.
- No changes to OPML import behaviour (it already auto-creates categories).
