mod common;
use common::default_test_config;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::{HeaderMap, HeaderName};
use axum_test::TestServer;
use rdrs::{
    auth, config::parse_trusted_networks, create_router, db,
    middleware::forward_auth::forward_auth_identity, services, AppState, Config, DbPool,
};
use rusqlite::Connection;

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

fn open_shared_memory(name: &str) -> Connection {
    let uri = format!("file:{}?mode=memory&cache=shared", name);
    Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap()
}

/// Build a server over a real loopback HTTP transport so the middleware sees a
/// genuine `ConnectInfo` peer (127.0.0.1). `trusted` controls whether loopback
/// is inside the trusted network. Each caller passes a unique `db_name` to
/// avoid cross-test interference when tests run in parallel.
fn create_server(db_name: &str, mut mutate: impl FnMut(&mut Config)) -> TestServer {
    let write_conn = open_shared_memory(db_name);
    db::init_db(&write_conn).unwrap();
    let read_conn = open_shared_memory(db_name);

    let mut config = default_test_config();
    config.auth_proxy_header = "Remote-User".to_string();
    config.auth_proxy_groups_header = "Remote-Groups".to_string();
    config.auth_proxy_admin_group = "admins".to_string();
    mutate(&mut config);

    let webauthn = auth::create_webauthn(&config).unwrap();
    let summary_cache = services::create_summary_cache(100, 24);
    let (summary_tx, _rx) = services::create_summary_channel(10);
    let (pool, _handle) = DbPool::new(write_conn, read_conn);
    let state = AppState {
        db: pool,
        config: Arc::new(config),
        webauthn: Arc::new(webauthn),
        summary_cache,
        summary_tx,
        sidebar_cache: Arc::new(services::SidebarCache::default()),
        summary_cancels: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        events: services::EventBus::new(16),
        shutdown: tokio_util::sync::CancellationToken::new(),
    };
    let app = create_router(state).into_make_service_with_connect_info::<SocketAddr>();
    TestServer::builder()
        .http_transport()
        .save_cookies()
        .build(app)
}

fn seed_user(db_name: &str, name: &str, role: rdrs::models::user::Role) {
    let conn = open_shared_memory(db_name);
    rdrs::models::user::create_user(&conn, name, "!", role).unwrap();
}

#[tokio::test]
async fn test_trusted_existing_user_gets_session() {
    let db_name = "fa_test_trusted_existing";
    let server = create_server(db_name, |c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    });
    seed_user(db_name, "alice", rdrs::models::user::Role::User);

    let res = server.get("/").add_header("Remote-User", "alice").await;

    // Redirect carrying a freshly-minted session cookie.
    assert!(res.status_code().is_redirection());
    assert!(res.maybe_cookie("session_token").is_some());
}

#[tokio::test]
async fn test_untrusted_peer_ignores_header() {
    let db_name = "fa_test_untrusted_peer";
    let server = create_server(db_name, |c| {
        // Loopback is NOT in this network → header must be ignored.
        c.trusted_proxy_networks = parse_trusted_networks("10.0.0.0/8").unwrap();
    });
    seed_user(db_name, "alice", rdrs::models::user::Role::User);

    let res = server.get("/").add_header("Remote-User", "alice").await;

    assert!(res.maybe_cookie("session_token").is_none());
}

#[tokio::test]
async fn test_unknown_user_creation_disabled_rejected() {
    let db_name = "fa_test_creation_disabled";
    let server = create_server(db_name, |c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
        c.auth_proxy_user_creation = false;
    });

    let res = server.get("/").add_header("Remote-User", "ghost").await;

    assert!(res.maybe_cookie("session_token").is_none());
    assert_eq!(res.header("location"), "/login");
}

#[tokio::test]
async fn test_unknown_user_jit_created_as_admin_via_groups() {
    let db_name = "fa_test_jit_admin_groups";
    let server = create_server(db_name, |c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
        c.auth_proxy_user_creation = true;
    });

    let res = server
        .get("/")
        .add_header("Remote-User", "bob")
        .add_header("Remote-Groups", "users,admins")
        .await;

    assert!(res.status_code().is_redirection());
    assert!(res.maybe_cookie("session_token").is_some());

    // Account was created with Admin role from the groups header.
    let conn = open_shared_memory(db_name);
    let created = rdrs::models::user::find_by_username(&conn, "bob")
        .unwrap()
        .unwrap();
    assert_eq!(created.role, rdrs::models::user::Role::Admin);
}

#[tokio::test]
async fn test_disabled_user_rejected() {
    let db_name = "fa_test_disabled_user";
    let server = create_server(db_name, |c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    });
    seed_user(db_name, "carol", rdrs::models::user::Role::User);
    let conn = open_shared_memory(db_name);
    let u = rdrs::models::user::find_by_username(&conn, "carol")
        .unwrap()
        .unwrap();
    rdrs::models::user::disable_user(&conn, u.id).unwrap();

    let res = server.get("/").add_header("Remote-User", "carol").await;

    assert!(res.maybe_cookie("session_token").is_none());
    assert_eq!(res.header("location"), "/login");
}

#[tokio::test]
async fn test_existing_user_role_recomputed_on_login() {
    let db_name = "fa_test_role_recompute";
    let server = create_server(db_name, |c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    });
    // Seed the user as Admin; the groups header will NOT include "admins".
    seed_user(db_name, "dave", rdrs::models::user::Role::Admin);

    let res = server
        .get("/")
        .add_header("Remote-User", "dave")
        .add_header("Remote-Groups", "users")
        .await;

    // Login must succeed with a session cookie.
    assert!(res.status_code().is_redirection());
    assert!(res.maybe_cookie("session_token").is_some());

    // Role must have been demoted from Admin → User.
    let conn = open_shared_memory(db_name);
    let user = rdrs::models::user::find_by_username(&conn, "dave")
        .unwrap()
        .unwrap();
    assert_eq!(user.role, rdrs::models::user::Role::User);
}

#[tokio::test]
async fn test_invalid_session_cookie_still_forward_auths() {
    let db_name = "fa_test_invalid_cookie";
    let server = create_server(db_name, |c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    });
    seed_user(db_name, "erin", rdrs::models::user::Role::User);

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
    let db_name = "fa_test_sidebar_via_forward_auth";
    let server = create_server(db_name, |c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    });
    seed_user(db_name, "grace", rdrs::models::user::Role::User);

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
async fn test_valid_session_cookie_not_reminted() {
    let db_name = "fa_test_valid_cookie_not_reminted";
    let server = create_server(db_name, |c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    });
    seed_user(db_name, "frank", rdrs::models::user::Role::User);

    // First login mints a valid session (saved by the client jar).
    server.get("/").add_header("Remote-User", "frank").await;

    // Second request carries the now-valid cookie: the validity check must
    // honour the existing session (pass through to the app) rather than
    // treating it as invalid and re-minting.  Guards against a regression where
    // the validity check wrongly rejects a good cookie, causing a redirect to
    // /login and/or a new cookie to be issued.
    let res = server.get("/").add_header("Remote-User", "frank").await;

    // The valid session must NOT be re-minted.
    let reminted = res
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|s| s.starts_with("session_token="));
    assert!(
        !reminted,
        "a valid session must not be re-minted on every request"
    );

    // The middleware must pass through to the app, not redirect to /login.
    // (A logged-in GET / may itself redirect within the app, but it must never
    // redirect to /login.)
    let location = res
        .maybe_header("location")
        .and_then(|v| v.to_str().ok().map(|s| s.to_string()))
        .unwrap_or_default();
    assert_ne!(
        location, "/login",
        "a valid session must not be redirected to /login"
    );
}
