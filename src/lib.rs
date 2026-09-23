use std::sync::Arc;

use axum::{
    Router,
    routing::{delete, get, post, put},
};
use tokio::sync::mpsc;
use tower_http::{compression::CompressionLayer, csrf::CsrfLayer, timeout::TimeoutLayer};
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
#[cfg(test)]
mod test_support;
pub mod utils;
pub mod version;

pub use config::Config;
pub use db::Db;
pub use middleware::auth::{SESSION_COOKIE_NAME, SESSION_COOKIE_NAME_HOST};
pub use models::{Role, User};
pub use utils::url_validation::FetchPolicy;
pub use version::GIT_VERSION;

use services::{SidebarCache, SummaryCache, SummaryJob};

/// Force mimalloc to return freed pages to the OS; call after bulk work to
/// collapse the resident spike on memory-constrained hosts.
pub fn reclaim_memory() {
    // SAFETY: `mi_collect` is thread-safe and only reclaims memory.
    #[allow(unsafe_code)]
    unsafe {
        libmimalloc_sys::mi_collect(true);
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Arc<Config>,
    /// The only path to URLs the app did not choose; vets every redirect hop
    /// and DNS answer. See [`services::fetch`].
    pub fetcher: services::Fetcher,
    pub webauthn: Arc<Webauthn>,
    pub summary_cache: Arc<SummaryCache>,
    pub summary_tx: mpsc::Sender<SummaryJob>,
    pub sidebar_cache: Arc<SidebarCache>,
    /// Memoized site-wide DB figures for `/statistics`; see
    /// [`services::page_cache::AdminDbStatsCache`].
    pub admin_db_stats_cache: services::AdminDbStatsCache,
    pub summary_cancels: services::CancelRegistry,
    pub summarizer_inflight: handlers::summarizer::InFlightRegistry,
    pub events: services::EventBus,
    pub shutdown: tokio_util::sync::CancellationToken,
    /// Per-client-IP throttle shared by every credential-accepting endpoint.
    /// See [`middleware::RateLimiter`].
    pub login_rate_limiter: Arc<crate::middleware::RateLimiter>,
}

pub fn create_router(state: AppState) -> Router {
    // The ETag/Date/Compression/Timeout layers would break SSE, so they wrap
    // `core` only.
    let core = Router::new()
        .route("/health", get(handlers::health::health_check))
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
        // Form POST sign-out for no-JS clients (forms cannot send DELETE).
        .route("/logout", post(handlers::auth::logout_form))
        // Anonymous: the path token is the only authority.
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
        // Form-encoded twin of `POST /api/session/reauth` for the routes below.
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
        .route(
            "/entries/offline",
            get(handlers::pages::offline_entries_page),
        )
        .route("/entries/{id}", get(handlers::pages::entry_page))
        .route(
            "/entries/{id}/fragment",
            get(handlers::entries::entry_fragment),
        )
        .route(
            "/entries/{id}/summary/fragment",
            get(handlers::entries::summary_fragment),
        )
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
        .route("/api/feeds/{id}/icon", get(handlers::feed::get_feed_icon))
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
        .route("/api/proxy/image", get(handlers::proxy::proxy_image))
        // Open-tracking pixel, authorised only by the HMAC in its path (fetchers
        // have no session). In the middleware skip lists as `/p/`.
        .route("/p/{token}", get(handlers::pixel::tracking_pixel))
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
        // Google Reader API, also under the FreshRSS-compatible prefix.
        .merge(handlers::greader::greader_routes())
        .nest("/api/greader.php", handlers::greader::greader_routes())
        .route("/static/{*path}", get(handlers::static_assets::serve))
        // PWA: `/sw.js` at the root for worker scope. Both routes are in the
        // session, CSRF and forward-auth skip lists to stay cookie-free.
        .route("/sw.js", get(handlers::static_assets::service_worker))
        .route("/offline", get(handlers::pages::offline_page))
        .fallback(handlers::pages::not_found_page)
        // `no-store` on session-bearing responses (OWASP). Inside `ETagLayer` so
        // it sees the handler's own `Cache-Control` first.
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
        // Synchronizer-token CSRF guard; innermost so it sees the cookie
        // `anonymous_session` may inject.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::csrf::csrf_guard,
        ))
        // Row-less session + CSRF cookie for logged-out visitors. Outside
        // `csrf_guard`, inside `forward_auth` so a real session wins.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::csrf::anonymous_session,
        ))
        // Slide the cookies' Max-Age with the server-side TTL (and rotate
        // periodically). Outside `anonymous_session` so it sees inner
        // Set-Cookies (notably logout's removals); inside `forward_auth` so it
        // never doubles up with it.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::auth::slide_session_cookie,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::forward_auth::forward_auth,
        ))
        // Sign/verify the flash cookie so handlers see plain JSON while the
        // browser holds only a signed value.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::flash::sign_flash_cookies,
        ))
        // First-line CSRF defence (header-only, position immaterial).
        .layer(CsrfLayer::new())
        // Directly outside the guard to read the `ProtectionError` on its 403.
        .layer(axum::middleware::from_fn(
            middleware::csrf::log_cross_site_rejection,
        ));

    let router = Router::new()
        // SSE lives outside the layers above.
        .route("/events", get(handlers::events::events_stream))
        .merge(core);

    // Fixed security headers; outermost for the same reason as HSTS below.
    let router = router.layer(axum::middleware::from_fn(middleware::set_security_headers));

    // HSTS, only for HTTPS deployments. Outermost over `core` and `/events`
    // because `forward_auth` and the CSRF guards short-circuit without `next`.
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

    // Per-request timing, outermost so every response is timed exactly once.
    let router = router.layer(axum::middleware::from_fn(
        middleware::request_log::log_request_duration,
    ));

    router.with_state(state)
}
