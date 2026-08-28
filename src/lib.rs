use std::sync::Arc;

use axum::{
    Router,
    routing::{delete, get, post, put},
};
use tokio::sync::mpsc;
use tower_http::{compression::CompressionLayer, timeout::TimeoutLayer};
use webauthn_rs::prelude::Webauthn;

use services::http::SERVER_REQUEST_TIMEOUT;

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod models;
pub mod secret;
pub mod services;
pub mod utils;
pub mod version;

pub use config::Config;
pub use db::Db;
pub use middleware::auth::{SESSION_COOKIE_NAME, SESSION_COOKIE_NAME_HOST};
pub use models::{Role, User};
pub use version::GIT_VERSION;

use services::{SidebarCache, SummaryCache, SummaryJob};

/// Force the allocator to return freed pages to the OS.
///
/// Bulk operations allocate large transient buffers that mimalloc would
/// otherwise hold for up to its purge delay. Calling this right after the bulk
/// work collapses the resident spike immediately, which matters on
/// memory-constrained hosts.
pub fn reclaim_memory() {
    // SAFETY: `mi_collect` is a thread-safe collection call with no
    // side-effects beyond reclaiming memory; `true` forces it to also return
    // memory to the OS.
    // Legitimate FFI call into mimalloc; no safe alternative exists.
    #[allow(unsafe_code)]
    unsafe {
        libmimalloc_sys::mi_collect(true);
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Arc<Config>,
    pub webauthn: Arc<Webauthn>,
    pub summary_cache: Arc<SummaryCache>,
    pub summary_tx: mpsc::Sender<SummaryJob>,
    pub sidebar_cache: Arc<SidebarCache>,
    pub summary_cancels: services::CancelRegistry,
    pub summarizer_inflight: handlers::summarizer::InFlightRegistry,
    pub events: services::EventBus,
    pub shutdown: tokio_util::sync::CancellationToken,
    /// Per-client-IP throttle shared by every credential-accepting endpoint
    /// (login, register, passkey authentication, `GReader` `ClientLogin`).
    /// See [`middleware::RateLimiter`] for why the check-and-count is a
    /// single locked operation rather than a separate check/record pair.
    pub login_rate_limiter: Arc<crate::middleware::RateLimiter>,
}

pub fn create_router(state: AppState) -> Router {
    // `core` holds every existing route. The ETag/Date/Compression/Timeout
    // layers below buffer the response body or abort after SERVER_REQUEST_TIMEOUT
    // — both fatal to a long-lived SSE stream — so they wrap `core` only.
    let core = Router::new()
        // Health check
        .route("/health", get(handlers::health::health_check))
        // Favicon routes
        .route("/favicon.ico", get(handlers::favicon::favicon_ico))
        .route("/favicon.svg", get(handlers::favicon::favicon_svg))
        .route("/favicon-16x16.png", get(handlers::favicon::favicon_16))
        .route("/favicon-32x32.png", get(handlers::favicon::favicon_32))
        .route(
            "/apple-touch-icon.png",
            get(handlers::favicon::apple_touch_icon),
        )
        .route("/", get(handlers::pages::unread_page))
        .route(
            "/login",
            get(handlers::pages::login_page).post(handlers::auth::login_form),
        )
        .route(
            "/setup",
            get(handlers::pages::setup_page).post(handlers::auth::setup_form),
        )
        // Sign-out as a form POST. `DELETE /api/session` is unreachable without
        // scripting — a form cannot send DELETE — which left a scriptless
        // reader unable to end their session at all.
        .route("/logout", post(handlers::auth::logout_form))
        // The one-time link an admin hands out. Anonymous by design: the token
        // in the path is the only authority, so nothing here reads a session.
        .route(
            "/invite/{token}",
            get(handlers::invite::invite_page).post(handlers::invite::redeem_form),
        )
        .route("/user-settings", get(handlers::pages::user_settings_page))
        .route("/admin", get(handlers::pages::admin_page))
        .route("/settings", get(handlers::pages::settings_page))
        .route("/api/setup", post(handlers::auth::setup))
        .route("/api/session", post(handlers::auth::login))
        .route("/api/session", delete(handlers::auth::logout))
        .route("/api/session/reauth", post(handlers::auth::reauthenticate))
        .route("/api/user", get(handlers::user::get_current_user))
        .route("/api/me", get(handlers::user::get_me))
        .route("/api/sidebar", get(handlers::user::get_sidebar))
        // The sync ledger for offline reading: which entries the browser
        // should be holding, and under what cache name.
        .route("/api/offline/manifest", get(handlers::offline::manifest))
        .route(
            "/api/sidebar/categories/{id}/feeds",
            get(handlers::user::get_sidebar_category_feeds),
        )
        .route("/api/user-settings", get(handlers::user::get_user_settings))
        .route("/api/user/settings/theme", get(handlers::user::get_theme))
        .route(
            "/api/user/settings/theme",
            put(handlers::user::update_theme),
        )
        // Form-action POST endpoints for the SSR /user-settings page (PR-4 T1).
        .route(
            "/user-settings/password",
            post(handlers::user::change_password_form),
        )
        .route(
            "/user-settings/preferences",
            post(handlers::user::update_preferences_form),
        )
        .route(
            "/user-settings/linkding",
            post(handlers::user::update_linkding_form),
        )
        .route(
            "/user-settings/kagi",
            post(handlers::user::update_kagi_form),
        )
        .route(
            "/user-settings/sessions/revoke-others",
            post(handlers::user::revoke_other_sessions_form),
        )
        .route(
            "/user-settings/sessions/{id}/revoke",
            post(handlers::user::revoke_session_form),
        )
        .route(
            "/user-settings/api-tokens/{id}/revoke",
            post(handlers::user::revoke_api_token_form),
        )
        .route(
            "/user-settings/api-tokens/revoke-all",
            post(handlers::user::revoke_all_api_tokens_form),
        )
        .route(
            "/api/admin/unmasquerade",
            post(handlers::admin::stop_masquerade),
        )
        // Form-action POST endpoints for the SSR /admin page (PR-5 T1).
        // `/admin/reauth` re-opens the confirmation window the four
        // account-changing routes below require; it is the form-encoded twin
        // of `POST /api/session/reauth`, which only `passkey.js` can drive.
        .route("/admin/reauth", post(handlers::admin::reauth_form))
        .route("/admin/users", post(handlers::admin::create_user_form))
        .route(
            "/admin/users/{id}/invite",
            post(handlers::admin::reissue_invite_form),
        )
        .route(
            "/admin/users/{id}/invite/revoke",
            post(handlers::admin::revoke_invite_form),
        )
        .route(
            "/admin/users/{id}/role",
            post(handlers::admin::update_role_form),
        )
        .route(
            "/admin/users/{id}/status",
            post(handlers::admin::update_status_form),
        )
        .route(
            "/admin/users/{id}/masquerade",
            post(handlers::admin::start_masquerade_form),
        )
        .route(
            "/admin/users/{id}/delete",
            post(handlers::admin::delete_user_form),
        )
        // Page routes
        .route(
            "/categories",
            get(handlers::pages::categories_page).post(handlers::categories::create_category_form),
        )
        .route(
            "/categories/{id}/rename",
            post(handlers::categories::rename_category_form),
        )
        .route(
            "/categories/{id}/delete",
            post(handlers::categories::delete_category_form),
        )
        .route(
            "/feeds",
            get(handlers::pages::feeds_page).post(handlers::feeds::create_feed_form),
        )
        .route(
            "/feeds/{id}/edit",
            get(handlers::pages::feed_edit_page).post(handlers::feeds::edit_feed_form),
        )
        .route(
            "/feeds/{id}/delete",
            post(handlers::feeds::delete_feed_form),
        )
        .route(
            "/feeds/{id}/refresh",
            post(handlers::feeds::refresh_feed_form),
        )
        .route(
            "/feeds/{id}/fetch-metadata",
            post(handlers::feeds::fetch_metadata_form),
        )
        .route(
            "/feeds/import",
            get(handlers::pages::feeds_import_page).post(handlers::feeds::import_opml_form),
        )
        .route("/entries", get(handlers::pages::entries_page))
        .route("/entries/read", get(handlers::pages::read_entries_page))
        .route(
            "/entries/starred",
            get(handlers::pages::starred_entries_page),
        )
        .route(
            "/entries/summarized",
            get(handlers::pages::summarized_entries_page),
        )
        // The reader's offline library, and the page the service worker falls
        // back to for a navigation it cannot reach the network for. A literal
        // segment, so axum's trie resolves it ahead of the `{id}` route below
        // regardless of registration order.
        .route(
            "/entries/offline",
            get(handlers::pages::offline_entries_page),
        )
        .route("/entries/{id}", get(handlers::pages::entry_page))
        // Fragment endpoint for the reading pane. Registered after /entries/{id} so
        // Axum's trie router resolves the literal `/fragment` segment before the
        // bare `{id}` parameter catch-all.
        .route(
            "/entries/{id}/fragment",
            get(handlers::entries::entry_fragment),
        )
        // Summary container fragment endpoint — re-renders only #rp-summary-container
        // for the SSE client. Registered before other /entries/{id}/... routes so
        // the literal `summary/fragment` segments resolve before the bare `{id}`
        // parameter route in axum's trie router.
        .route(
            "/entries/{id}/summary/fragment",
            get(handlers::entries::summary_fragment),
        )
        // Star / read toggle action endpoints. Return multi-target HTML
        // (`_entry_actions_multi.html`) swapping the row + sidebar-unread.
        // Registered after /entries/{id}/fragment so literal path segments
        // (`star`, `read`) resolve before any future `{action}` wildcard.
        .route(
            "/entries/{id}/star",
            post(handlers::entries::star_entry_form),
        )
        .route(
            "/entries/{id}/unstar",
            post(handlers::entries::unstar_entry_form),
        )
        .route(
            "/entries/{id}/read",
            post(handlers::entries::read_entry_form),
        )
        .route(
            "/entries/{id}/unread",
            post(handlers::entries::unread_entry_form),
        )
        .route(
            "/entries/{id}/summarize",
            post(handlers::entries::summarize_entry_form),
        )
        .route(
            "/entries/{id}/summarize/cancel",
            post(handlers::entries::summarize_cancel_form),
        )
        .route(
            "/entries/{id}/fetch-full-content",
            post(handlers::entries::fetch_full_content_form),
        )
        .route(
            "/entries/{id}/save",
            post(handlers::entries::save_entry_form),
        )
        .route("/search", get(handlers::pages::search_page))
        .route("/statistics", get(handlers::pages::statistics_page))
        .route(
            "/summarizer",
            get(handlers::summarizer::summarizer_page).post(handlers::summarizer::start),
        )
        .route("/summarizer/item", post(handlers::summarizer::item))
        .route(
            "/categories/{id}/entries",
            get(handlers::pages::category_entries_page),
        )
        .route(
            "/categories/{id}/entries/mark-read",
            post(handlers::pages::category_mark_read_form),
        )
        .route(
            "/feeds/{id}/entries",
            get(handlers::pages::feed_entries_page),
        )
        .route(
            "/feeds/{id}/entries/mark-read",
            post(handlers::pages::feed_mark_read_form),
        )
        // RDRS-specific feed endpoints (not replaced by GReader API).
        // Icon URL is referenced from `<img src="…">` in the SSR /feeds page;
        // mutation goes through the form-action endpoints under /feeds/*.
        .route("/api/feeds/{id}/icon", get(handlers::feed::get_feed_icon))
        // RDRS-specific entry endpoints (not replaced by GReader API)
        .route(
            "/api/entries/{id}/fetch-full-content",
            post(handlers::entry::fetch_full_content),
        )
        .route(
            "/api/entries/{id}/save",
            post(handlers::entry::save_to_services),
        )
        .route(
            "/api/entries/{id}/summarize",
            post(handlers::entry::summarize_entry),
        )
        .route(
            "/api/entries/{id}/summary",
            get(handlers::entry::get_entry_summary),
        )
        .route(
            "/api/entries/{id}/summary",
            delete(handlers::entry::delete_entry_summary),
        )
        .route(
            "/api/entries/{id}/neighbors",
            get(handlers::entry::get_entry_neighbors),
        )
        // Proxy routes
        .route("/api/proxy/image", get(handlers::proxy::proxy_image))
        // Passkey routes
        .route(
            "/api/passkey/register/start",
            post(handlers::passkey::start_registration),
        )
        .route(
            "/api/passkey/register/finish",
            post(handlers::passkey::finish_registration),
        )
        .route(
            "/api/passkey/auth/start",
            post(handlers::passkey::start_authentication),
        )
        .route(
            "/api/passkey/auth/finish",
            post(handlers::passkey::finish_authentication),
        )
        .route("/api/passkeys", get(handlers::passkey::list_passkeys))
        .route("/api/passkeys/{id}", put(handlers::passkey::rename_passkey))
        .route(
            "/api/passkeys/{id}",
            delete(handlers::passkey::delete_passkey),
        )
        // Google Reader API (standard paths + FreshRSS-compatible /api/greader.php prefix)
        .merge(handlers::greader::greader_routes())
        .nest("/api/greader.php", handlers::greader::greader_routes())
        .route("/static/{*path}", get(handlers::static_assets::serve))
        // PWA. `/sw.js` sits at the root because a worker's scope is the
        // directory it was served from, and `/offline` beside it because the
        // worker precaches it as the fallback for a navigation that never
        // reaches us. Both are in the session, CSRF and forward-auth skip lists
        // so they stay cookie-free and publicly cacheable.
        .route("/sw.js", get(handlers::static_assets::service_worker))
        .route("/offline", get(handlers::pages::offline_page))
        .fallback(handlers::pages::not_found_page)
        // Mark session-bearing responses `no-store` (OWASP: Web Content Caching)
        // so a browser disk cache or shared proxy cannot replay a logged-in page.
        // Layered inside `ETagLayer` so it observes the handler's own
        // `Cache-Control` — the deliberate public-caching call sites — before
        // ETag processing runs; see cache_control.rs for why `no-store` also
        // makes ETag a no-op for the responses it does touch.
        .layer(axum::middleware::from_fn(
            middleware::cache_control::no_store_for_authenticated,
        ));

    let core = core
        .layer(middleware::ETagLayer::new())
        .layer(middleware::DateHeaderLayer::new())
        .layer(CompressionLayer::new().gzip(true).br(true))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            SERVER_REQUEST_TIMEOUT,
        ))
        // Synchronizer-token CSRF guard (second line): runs just before the
        // handler so it sees the session cookie `anonymous_session` may have
        // injected on this same request.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::csrf::csrf_guard,
        ))
        // Mint a signed (row-less) session + readable CSRF cookie for a
        // logged-out visitor, so every form — login and register included —
        // carries a token. Layered outside `csrf_guard` so the guard sees the
        // cookie, and inside `forward_auth` so a real forward-auth session wins.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::csrf::anonymous_session,
        ))
        // Reissue the session + CSRF cookies' Max-Age on every authenticated
        // request, so a still-in-use browser session tracks the sliding
        // server-side TTL instead of expiring on a fixed schedule. Their *value*
        // changes only when this layer also performs the periodic token rotation.
        //
        // Layered outside `anonymous_session` so it sees — and can correctly skip
        // re-setting — the Set-Cookies that layer and the handlers beneath emit,
        // most importantly `logout`'s removals; and inside `forward_auth`, which
        // short-circuits without calling `next` on every path that mints a
        // cookie, so this layer never doubles up with it.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::auth::slide_session_cookie,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::forward_auth::forward_auth,
        ))
        // First-line CSRF defence: reject provably cross-site state-changing
        // requests. Header-only and stateless, so its position in the stack is
        // immaterial; the synchronizer-token guard is layered on separately.
        .layer(axum::middleware::from_fn(
            middleware::csrf::csrf_origin_guard,
        ));

    let router = Router::new()
        // SSE lives outside the layers above. It still gets `state` via the
        // shared `.with_state` below.
        .route("/events", get(handlers::events::events_stream))
        .merge(core);

    // The fixed security headers (CSP, nosniff, Referrer-Policy,
    // Permissions-Policy, X-Frame-Options, COOP). Unconditional, and outermost
    // for the same reason HSTS is below — see middleware::security_headers for
    // what each directive is doing and what is deliberately absent.
    let router = router.layer(axum::middleware::from_fn(middleware::set_security_headers));

    // Strict-Transport-Security: only added when `Config` says the deployment is
    // HTTPS. The header value is built once here, where `config` is in scope,
    // rather than per response — and when it's `None` (the default) no layer is
    // added at all, so a plain-HTTP deployment pays nothing.
    //
    // Applied last, i.e. outermost over both `core` and `/events`, because
    // `forward_auth` and the CSRF guards short-circuit without calling `next` on
    // several paths, so a layer nested inside them would never see those
    // responses; `/events` sits outside `core` entirely.
    let router = if let Some(header_value) = state.config.hsts_header_value() {
        let value = axum::http::HeaderValue::from_str(&header_value)
            .expect("hsts_header_value only ever produces a valid header value");
        router.layer(axum::middleware::from_fn_with_state(
            middleware::HstsState::new(value),
            middleware::set_hsts,
        ))
    } else {
        router
    };

    // Per-request timing. Outermost — over the security headers, both CSRF guards
    // and `forward_auth`, all of which answer some requests without calling
    // `next`, and over `/events` — so every response is timed exactly once. See
    // middleware::request_log for what the duration does and does not include.
    let router = router.layer(axum::middleware::from_fn(
        middleware::request_log::log_request_duration,
    ));

    router.with_state(state)
}
