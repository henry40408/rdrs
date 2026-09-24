//! Where a form POST sends the user back to, from an untrusted `return_to`
//! field or query parameter.

/// `raw` as a same-origin path and query, if it is a rooted path that `allow`
/// accepts after normalisation (`/feeds/../admin` is checked as `/admin`).
/// Scheme-relative (`//host`), backslash and control-character tricks are
/// rejected, so the result can never leave this origin.
pub fn safe_return_to(raw: &str, allow: impl Fn(&str) -> bool) -> Option<String> {
    if !raw.starts_with('/')
        || raw.starts_with("//")
        || raw.contains('\\')
        || raw.chars().any(char::is_control)
    {
        return None;
    }
    let base = url::Url::parse("http://return-to.invalid/").ok()?;
    let url = base.join(raw).ok()?;
    if url.origin() != base.origin() || !allow(url.path()) {
        return None;
    }
    Some(path_and_query(&url))
}

/// `url`'s path and query, dropping scheme, host and fragment.
pub fn path_and_query(url: &url::Url) -> String {
    match url.query() {
        Some(q) => format!("{}?{}", url.path(), q),
        None => url.path().to_string(),
    }
}

/// `return_to` as a `key=value` query string, for links that carry it along.
pub fn return_to_query(return_to: &str) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .append_pair("return_to", return_to)
        .finish()
}

/// The bare feeds list, where feed forms land without a usable `return_to`.
pub const FEEDS_LIST: &str = "/feeds";

/// The (filtered) feeds list a feed form came from, else [`FEEDS_LIST`].
pub fn feeds_list(raw: Option<&str>) -> String {
    raw.and_then(|r| safe_return_to(r, |path| path == FEEDS_LIST))
        .unwrap_or_else(|| FEEDS_LIST.to_string())
}

/// Where sign-in may send the user: any page except the signed-out flows and
/// the machine endpoints, which would loop or render nothing useful.
pub fn is_login_destination(path: &str) -> bool {
    const EXCLUDED: [&str; 7] = [
        "/login", "/logout", "/setup", "/invite", "/api", "/reader", "/static",
    ];
    !EXCLUDED.iter().any(|prefix| {
        path.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feeds_only(path: &str) -> bool {
        path == "/feeds"
    }

    #[test]
    fn keeps_an_allowed_path_and_its_query() {
        assert_eq!(
            safe_return_to("/feeds?category=3&sort=unread", feeds_only).as_deref(),
            Some("/feeds?category=3&sort=unread")
        );
        assert_eq!(
            safe_return_to("/feeds", feeds_only).as_deref(),
            Some("/feeds")
        );
    }

    #[test]
    fn drops_the_fragment() {
        assert_eq!(
            safe_return_to("/feeds?filter=errors#row-feed-1", feeds_only).as_deref(),
            Some("/feeds?filter=errors")
        );
    }

    #[test]
    fn rejects_anything_that_could_leave_the_origin_or_the_allowlist() {
        for raw in [
            "",
            "feeds",
            "//evil.example/feeds",
            "/\\evil.example/feeds",
            "/feeds\\..\\admin",
            "https://evil.example/feeds",
            "javascript:alert(1)",
            "/feeds\r\nSet-Cookie: x=1",
            "/feeds\t",
            "/admin",
            "/feeds/../admin",
            "/feeds/1/edit",
        ] {
            assert_eq!(safe_return_to(raw, feeds_only), None, "{raw:?}");
        }
    }

    #[test]
    fn feeds_list_falls_back_to_the_bare_list() {
        assert_eq!(feeds_list(None), "/feeds");
        assert_eq!(feeds_list(Some("//evil.example")), "/feeds");
        assert_eq!(feeds_list(Some("/admin")), "/feeds");
        assert_eq!(
            feeds_list(Some("/feeds?filter=stale")),
            "/feeds?filter=stale"
        );
    }

    #[test]
    fn login_destinations_exclude_signed_out_and_machine_routes() {
        for path in [
            "/",
            "/feeds",
            "/entries/starred",
            "/admin",
            "/loginfo",
            "/apis",
        ] {
            assert!(is_login_destination(path), "{path}");
        }
        for path in [
            "/login",
            "/logout",
            "/setup",
            "/invite/abc",
            "/api/session",
            "/reader/api/0/stream/contents",
            "/static/js/app.js",
        ] {
            assert!(!is_login_destination(path), "{path}");
        }
    }

    #[test]
    fn return_to_query_round_trips_through_safe_return_to() {
        let original = "/feeds?category=3&sort=unread&filter=all";
        let query = return_to_query(original);
        assert_eq!(
            query,
            "return_to=%2Ffeeds%3Fcategory%3D3%26sort%3Dunread%26filter%3Dall"
        );
        let decoded = url::form_urlencoded::parse(query.as_bytes())
            .find(|(k, _)| k == "return_to")
            .map(|(_, v)| v.into_owned())
            .unwrap();
        assert_eq!(
            safe_return_to(&decoded, feeds_only).as_deref(),
            Some(original)
        );
    }
}
