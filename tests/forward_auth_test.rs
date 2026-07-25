mod common;
use common::{apply_csrf, default_test_config};

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum_test::TestServer;
use rdrs::{
    AppState, Config, Db, auth, config::parse_trusted_networks, create_router,
    middleware::forward_auth::forward_auth_identity, services,
};
use serde_json::json;

fn header_map(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.insert(
            HeaderName::from_bytes(k.as_bytes()).unwrap(),
            v.parse().unwrap(),
        );
    }
    h
}

#[test]
fn test_forward_auth_identity() {
    let mut cfg = default_test_config();
    cfg.auth_proxy_header = "Remote-User".to_string();
    cfg.trusted_proxy_networks = parse_trusted_networks("10.0.0.0/8").unwrap();

    let trusted: std::net::IpAddr = "10.1.2.3".parse().unwrap();
    let untrusted: std::net::IpAddr = "192.168.0.1".parse().unwrap();
    let with_header = header_map(&[("Remote-User", "alice")]);

    // trusted peer + header present → identity
    assert_eq!(
        forward_auth_identity(&cfg, Some(trusted), &with_header),
        Some("alice".to_string())
    );
    // untrusted peer → None
    assert_eq!(
        forward_auth_identity(&cfg, Some(untrusted), &with_header),
        None
    );
    // no peer IP → None
    assert_eq!(forward_auth_identity(&cfg, None, &with_header), None);
    // header missing → None
    assert_eq!(
        forward_auth_identity(&cfg, Some(trusted), &HeaderMap::new()),
        None
    );
    // header empty → None
    assert_eq!(
        forward_auth_identity(&cfg, Some(trusted), &header_map(&[("Remote-User", "  ")])),
        None
    );

    // feature off (empty header name) → None
    let mut off = cfg.clone();
    off.auth_proxy_header = String::new();
    assert_eq!(
        forward_auth_identity(&off, Some(trusted), &with_header),
        None
    );
}

/// Build a server over a real loopback HTTP transport so the middleware sees a
/// genuine `ConnectInfo` peer (127.0.0.1). `trusted` controls whether loopback
/// is inside the trusted network. Returns the backing `Db` so callers can seed
/// and inspect users directly.
async fn create_server(mut mutate: impl FnMut(&mut Config)) -> (TestServer, Db) {
    let db = Db::connect_in_memory().await.unwrap();

    let mut config = default_test_config();
    config.auth_proxy_header = "Remote-User".to_string();
    config.auth_proxy_groups_header = "Remote-Groups".to_string();
    config.auth_proxy_admin_group = "admins".to_string();
    mutate(&mut config);

    let webauthn = auth::create_webauthn(&config).unwrap();
    let summary_cache = services::create_summary_cache(100, 24);
    let (summary_tx, _rx) = services::create_summary_channel(10);
    let state = AppState {
        db: db.clone(),
        config: Arc::new(config),
        webauthn: Arc::new(webauthn),
        summary_cache,
        summary_tx,
        sidebar_cache: Arc::new(services::SidebarCache::default()),
        summary_cancels: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        summarizer_inflight: rdrs::handlers::summarizer::new_inflight_registry(),
        events: services::EventBus::new(16),
        shutdown: tokio_util::sync::CancellationToken::new(),
        login_rate_limiter: common::test_rate_limiter(),
    };
    let app = create_router(state).into_make_service_with_connect_info::<SocketAddr>();
    let server = TestServer::builder()
        .http_transport()
        .save_cookies()
        .build(app);
    (server, db)
}

async fn seed_user(db: &Db, name: &str, role: rdrs::models::user::Role) {
    rdrs::models::user::create_user(db, name, "!", role)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_trusted_existing_user_gets_session() {
    let (server, db) = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    })
    .await;
    seed_user(&db, "alice", rdrs::models::user::Role::User).await;

    let res = server.get("/").add_header("Remote-User", "alice").await;

    // Redirect carrying a freshly-minted session cookie.
    assert!(res.status_code().is_redirection());
    assert!(res.maybe_cookie("session_token").is_some());
}

#[tokio::test]
async fn test_untrusted_peer_ignores_header() {
    let (server, db) = create_server(|c| {
        // Loopback is NOT in this network → header must be ignored.
        c.trusted_proxy_networks = parse_trusted_networks("10.0.0.0/8").unwrap();
    })
    .await;
    seed_user(&db, "alice", rdrs::models::user::Role::User).await;

    let res = server.get("/").add_header("Remote-User", "alice").await;

    // The untrusted peer's header is ignored, so no forward-auth session is
    // established: the request stays unauthenticated and the protected home page
    // redirects it to /login. A logged-out visitor still receives an anonymous,
    // DB-less `session_token` from `anonymous_session` (it only carries a CSRF
    // token, backs no `session` row), so the meaningful assertion is that they
    // were *not* authenticated as alice — not the mere absence of the cookie.
    assert!(res.status_code().is_redirection());
    assert_eq!(res.header("location"), "/login");
}

#[tokio::test]
async fn test_unknown_user_creation_disabled_rejected() {
    let (server, _db) = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
        c.auth_proxy_user_creation = false;
    })
    .await;

    let res = server.get("/").add_header("Remote-User", "ghost").await;

    assert!(res.maybe_cookie("session_token").is_none());
    assert_eq!(res.header("location"), "/login");
}

#[tokio::test]
async fn test_unknown_user_jit_created_as_admin_via_groups() {
    let (server, db) = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
        c.auth_proxy_user_creation = true;
    })
    .await;

    let res = server
        .get("/")
        .add_header("Remote-User", "bob")
        .add_header("Remote-Groups", "users,admins")
        .await;

    assert!(res.status_code().is_redirection());
    assert!(res.maybe_cookie("session_token").is_some());

    // Account was created with Admin role from the groups header.
    let created = rdrs::models::user::find_by_username(&db, "bob")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(created.role, rdrs::models::user::Role::Admin);
}

#[tokio::test]
async fn test_disabled_user_rejected() {
    let (server, db) = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    })
    .await;
    seed_user(&db, "carol", rdrs::models::user::Role::User).await;
    let u = rdrs::models::user::find_by_username(&db, "carol")
        .await
        .unwrap()
        .unwrap();
    rdrs::models::user::disable_user(&db, u.id).await.unwrap();

    let res = server.get("/").add_header("Remote-User", "carol").await;

    assert!(res.maybe_cookie("session_token").is_none());
    assert_eq!(res.header("location"), "/login");
}

#[tokio::test]
async fn test_existing_user_role_recomputed_on_login() {
    let (server, db) = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    })
    .await;
    // Seed the user as Admin; the groups header will NOT include "admins".
    seed_user(&db, "dave", rdrs::models::user::Role::Admin).await;

    let res = server
        .get("/")
        .add_header("Remote-User", "dave")
        .add_header("Remote-Groups", "users")
        .await;

    // Login must succeed with a session cookie.
    assert!(res.status_code().is_redirection());
    assert!(res.maybe_cookie("session_token").is_some());

    // Role must have been demoted from Admin → User.
    let user = rdrs::models::user::find_by_username(&db, "dave")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.role, rdrs::models::user::Role::User);
}

#[tokio::test]
async fn test_invalid_session_cookie_still_forward_auths() {
    let (server, db) = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    })
    .await;
    seed_user(&db, "erin", rdrs::models::user::Role::User).await;

    // A stale/garbage session_token cookie must NOT block forward-auth.
    let res = server
        .get("/")
        .add_header("Remote-User", "erin")
        .add_cookie(cookie::Cookie::new("session_token", "stale-invalid"))
        .await;

    assert!(res.status_code().is_redirection());
    let fresh = res
        .maybe_cookie("session_token")
        .expect("a fresh session cookie should be minted");
    assert_ne!(fresh.value(), "stale-invalid");
    assert!(!fresh.value().is_empty());
}

#[tokio::test]
async fn test_sidebar_reports_via_forward_auth_dynamically() {
    let (server, db) = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    })
    .await;
    seed_user(&db, "grace", rdrs::models::user::Role::User).await;

    // Establish a session via forward-auth (page route mints the cookie).
    server.get("/").add_header("Remote-User", "grace").await;

    // With the proxy header present → via_forward_auth true.
    let with = server
        .get("/api/sidebar")
        .add_header("Remote-User", "grace")
        .await;
    with.assert_status_ok();
    assert_eq!(with.json::<serde_json::Value>()["via_forward_auth"], true);

    // Same valid session, but no proxy header on this request → false (dynamic).
    let without = server.get("/api/sidebar").await;
    without.assert_status_ok();
    assert_eq!(
        without.json::<serde_json::Value>()["via_forward_auth"],
        false
    );
}

#[tokio::test]
async fn test_admin_page_sidebar_bootstrap_reports_via_forward_auth() {
    let (server, db) = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    })
    .await;
    seed_user(&db, "heidi", rdrs::models::user::Role::Admin).await;

    // Establish session via forward-auth; include the admin group so the role
    // is not demoted (create_server sets group_mapping_enabled with "admins").
    server
        .get("/")
        .add_header("Remote-User", "heidi")
        .add_header("Remote-Groups", "admins")
        .await;

    // Fetch /admin with the proxy header present.
    let res = server
        .get("/admin")
        .add_header("Remote-User", "heidi")
        .add_header("Remote-Groups", "admins")
        .await;
    res.assert_status_ok();

    // The embedded rdrs-sidebar-bootstrap JSON must reflect via_forward_auth: true.
    let body = res.text();
    assert!(
        body.contains("\"via_forward_auth\":true"),
        "expected via_forward_auth:true in sidebar bootstrap JSON, got body snippet: {:?}",
        body.get(0..500).unwrap_or(&body)
    );
}

#[tokio::test]
async fn test_valid_session_cookie_not_reminted() {
    let (server, db) = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    })
    .await;
    seed_user(&db, "frank", rdrs::models::user::Role::User).await;

    // First login mints a valid session (saved by the client jar).
    let first = server.get("/").add_header("Remote-User", "frank").await;
    let original_value = first
        .maybe_cookie("session_token")
        .expect("forward-auth login must mint a session cookie")
        .value()
        .to_string();

    // Second request carries the now-valid cookie: the validity check must
    // honour the existing session (pass through to the app) rather than
    // treating it as invalid and re-minting a *different* one. Guards against
    // a regression where the validity check wrongly rejects a good cookie,
    // causing a redirect to /login and/or a brand-new session to be issued.
    //
    // The sliding-TTL middleware (layered inside forward_auth) is expected to
    // still reissue *this same* cookie on the pass-through to refresh its
    // Max-Age, so the meaningful assertion is that the token value never
    // changes — not that no Set-Cookie appears at all.
    let res = server.get("/").add_header("Remote-User", "frank").await;

    if let Some(reissued) = res.maybe_cookie("session_token") {
        assert_eq!(
            reissued.value(),
            original_value,
            "a valid session must not be re-minted with a different token"
        );
    }

    // The middleware must pass through to the app, not redirect to /login.
    // (A logged-in GET / may itself redirect within the app, but it must never
    // redirect to /login.)
    let location = res
        .maybe_header("location")
        .and_then(|v| v.to_str().ok().map(std::string::ToString::to_string))
        .unwrap_or_default();
    assert_ne!(
        location, "/login",
        "a valid session must not be redirected to /login"
    );
}

/// `forward_auth` short-circuits with its own redirect response *without*
/// calling `next` whenever it mints a fresh session — so `slide_session_cookie`,
/// layered inside it, never runs on that response and cannot double up with
/// `forward_auth`'s own `Set-Cookie`. This asserts exactly that: the very first
/// forward-auth login response carries exactly one `session_token` Set-Cookie.
#[tokio::test]
async fn test_forward_auth_cookie_is_not_overwritten_by_slide() {
    let (server, db) = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    })
    .await;
    seed_user(&db, "olga", rdrs::models::user::Role::User).await;

    let res = server.get("/").add_header("Remote-User", "olga").await;
    assert!(res.status_code().is_redirection());

    let session_cookies: Vec<_> = res
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter(|s| s.starts_with("session_token="))
        .collect();
    assert_eq!(
        session_cookies.len(),
        1,
        "forward_auth's own Set-Cookie must not be doubled up by the sliding \
         middleware, since forward_auth doesn't call `next` on this path: {session_cookies:?}"
    );
}

#[tokio::test]
async fn test_logout_reports_via_forward_auth() {
    let (mut server, db) = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    })
    .await;
    seed_user(&db, "ivan", rdrs::models::user::Role::User).await;

    // Establish a session via forward-auth; the response also mints the
    // readable CSRF cookie the DELETE below needs.
    let login = server.get("/").add_header("Remote-User", "ivan").await;
    apply_csrf(&mut server, &login);

    // No `auth_proxy_logout_url` is configured, and the proxy header is
    // present again on this request, so AuthUser must report
    // via_forward_auth: true — the client uses this to avoid claiming a
    // local logout ended a session the proxy will just re-mint.
    let res = server
        .delete("/api/session")
        .add_header("Remote-User", "ivan")
        .await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["via_forward_auth"], true);
    assert_eq!(body["redirect_to"], "/login");
    assert_eq!(body["logout_url_configured"], false);
}

#[tokio::test]
async fn test_logout_password_session_reports_via_forward_auth_false() {
    // Same server config as above (forward-auth configured, loopback
    // trusted), but this session is a normal local password login and the
    // logout request carries no proxy identity header. via_forward_auth must
    // be false: a trusted peer alone must not be enough to claim the session
    // came from the proxy.
    let (mut server, _db) = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    })
    .await;

    server
        .post("/api/register")
        .json(&json!({ "username": "judy", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);
    let login = server
        .post("/api/session")
        .json(&json!({ "username": "judy", "password": "password123" }))
        .await;
    login.assert_status_ok();
    apply_csrf(&mut server, &login);

    let res = server.delete("/api/session").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["via_forward_auth"], false);
    assert_eq!(body["redirect_to"], "/login");
    assert_eq!(body["logout_url_configured"], false);
}
