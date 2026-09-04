//! The open-tracking pixel: a 1x1 image appended to rendered entry content so
//! the server learns which entries a client actually rendered.
//!
//! **Injection must run on the output of [`sanitize_html`], never on its
//! input.** The sanitiser removes 1x1 images outright (`is_tracking_pixel`) and
//! rewrites every surviving `<img src>` through the signed image proxy. Our
//! pixel is exactly a 1x1 image and must stay same-origin, so both of those
//! would destroy it. The ordering is not a nicety — it is the only thing that
//! makes the feature work, and it is pinned by a test in each call site's file
//! as well as by `injected_pixel_survives_the_sanitiser` below.
//!
//! [`sanitize_html`]: crate::services::sanitize_html

use chrono::{DateTime, Utc};

use crate::secret;

/// The 43-byte transparent GIF every pixel request is answered with, valid or
/// not. Inlined rather than fetched from `static/` because the response must
/// not depend on anything the request can influence.
pub const TRANSPARENT_GIF: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, // "GIF89a"
    0x01, 0x00, 0x01, 0x00, // 1x1
    0x80, 0x00, 0x00, // global colour table, 2 entries
    0x00, 0x00, 0x00, // colour 0: black (the transparent one)
    0xFF, 0xFF, 0xFF, // colour 1: white
    0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, // graphic control: index 0 transparent
    0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, // image descriptor
    0x02, 0x02, 0x44, 0x01, 0x00, // LZW image data
    0x3B, // trailer
];

/// Path prefix the pixel endpoint is mounted under.
///
/// Deliberately short and outside `/api`: the URL is embedded in entry HTML that
/// external readers cache, and the `.gif` suffix is what lets a client treat the
/// response as an ordinary image. The middleware skip lists in
/// `middleware::forward_auth` and `middleware::csrf` name this prefix for the
/// same reason `/api` appears in them — the request carries no session and must
/// not be given one.
pub const PIXEL_PATH_PREFIX: &str = "/p/";

/// Everything the render paths need to decide on, address and sign a pixel.
///
/// Built once per request rather than per entry: a `GReader` page serialises up
/// to a thousand items, and reading `pixel_tracking_enabled_at` for each would
/// turn one settings lookup into a thousand.
#[derive(Debug, Clone, Copy)]
pub struct PixelContext<'a> {
    pub user_id: i64,
    /// When this reader opted in, or `None` for opted out.
    pub enabled_at: Option<DateTime<Utc>>,
    /// The root key; the pixel signature derives from it under
    /// [`secret::DOMAIN_PIXEL`].
    pub secret: &'a [u8],
    /// Absolute base for the pixel URL, for content that will be rendered
    /// outside this origin (`GReader` clients). `None` yields a root-relative
    /// URL, which is what an in-page render wants.
    pub base_url: Option<&'a str>,
}

impl<'a> PixelContext<'a> {
    /// An opted-out context, for the render paths that have no reader-specific
    /// settings to hand.
    pub fn disabled(user_id: i64, secret: &'a [u8]) -> Self {
        Self {
            user_id,
            enabled_at: None,
            secret,
            base_url: None,
        }
    }

    /// Whether this reader is tracking opens at all.
    pub fn is_enabled(&self) -> bool {
        self.enabled_at.is_some()
    }

    /// The pixel URL for one entry.
    pub fn url(&self, entry_id: i64) -> String {
        let sig = secret::pixel_sig(self.secret, self.user_id, entry_id);
        let path = format!("{PIXEL_PATH_PREFIX}{}-{entry_id}-{sig}.gif", self.user_id);
        match self.base_url {
            Some(base) => format!("{}{}", base.trim_end_matches('/'), path),
            None => path,
        }
    }

    /// The `<img>` tag appended to an entry's rendered HTML.
    fn img_tag(&self, entry_id: i64) -> String {
        format!(
            r#"<img src="{}" width="1" height="1" alt="" aria-hidden="true">"#,
            self.url(entry_id)
        )
    }

    /// Append the pixel to `html` when this reader is tracking opens and the
    /// entry is one the metric can speak about; otherwise hand `html` back
    /// untouched.
    ///
    /// `entry_created_at` gates on the opt-in baseline for the same reason the
    /// endpoint re-checks it: an entry that arrived before tracking was turned
    /// on is not in the denominator, so serving it a pixel would only produce a
    /// request the endpoint then throws away.
    ///
    /// # Ordering
    ///
    /// Call this on the *result* of `sanitize_html`. See the module docs.
    pub fn maybe_inject(
        &self,
        mut html: String,
        entry_id: i64,
        entry_created_at: DateTime<Utc>,
    ) -> String {
        let Some(enabled_at) = self.enabled_at else {
            return html;
        };
        if entry_created_at < enabled_at {
            return html;
        }
        // Empty content stays empty: a pane with nothing in it renders no
        // article, so there was no open to record and a lone pixel would only
        // give the empty state something to lay out around.
        if html.trim().is_empty() {
            return html;
        }
        html.push_str(&self.img_tag(entry_id));
        html
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::sanitize_html;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    fn ctx(enabled_at: Option<DateTime<Utc>>) -> PixelContext<'static> {
        PixelContext {
            user_id: 7,
            enabled_at,
            secret: SECRET,
            base_url: None,
        }
    }

    fn hour_ago() -> DateTime<Utc> {
        Utc::now() - chrono::Duration::hours(1)
    }

    #[test]
    fn opted_out_content_is_untouched() {
        let html = "<p>Article</p>".to_string();
        assert_eq!(
            ctx(None).maybe_inject(html.clone(), 42, Utc::now()),
            html,
            "no pixel may be served to a reader who never opted in"
        );
    }

    #[test]
    fn opted_in_content_gets_a_verifiable_pixel() {
        let out = ctx(Some(hour_ago())).maybe_inject("<p>Article</p>".to_string(), 42, Utc::now());
        assert!(out.starts_with("<p>Article</p>"), "{out}");
        assert!(out.contains(r#"width="1" height="1""#), "{out}");

        let sig = out
            .split("/p/7-42-")
            .nth(1)
            .and_then(|rest| rest.split(".gif").next())
            .expect("pixel URL carries user, entry and signature");
        assert!(secret::verify_pixel_sig(SECRET, 7, 42, sig));
    }

    #[test]
    fn entries_older_than_the_opt_in_get_no_pixel() {
        // The backlog that was already in the database when tracking was turned
        // on is outside the denominator, so it must not be asked to report.
        let enabled_at = Utc::now();
        let html = "<p>Article</p>".to_string();
        assert_eq!(
            ctx(Some(enabled_at)).maybe_inject(
                html.clone(),
                42,
                enabled_at - chrono::Duration::seconds(1)
            ),
            html
        );
    }

    #[test]
    fn empty_content_stays_empty() {
        assert_eq!(
            ctx(Some(hour_ago())).maybe_inject(String::new(), 42, Utc::now()),
            ""
        );
    }

    #[test]
    fn base_url_makes_the_pixel_absolute_for_external_clients() {
        let mut c = ctx(Some(hour_ago()));
        c.base_url = Some("https://rdrs.example.com/");
        let out = c.maybe_inject("<p>Article</p>".to_string(), 42, Utc::now());
        assert!(
            out.contains(r#"src="https://rdrs.example.com/p/7-42-"#),
            "{out}"
        );
        // One slash, not two — the trailing slash on the base is trimmed.
        assert!(!out.contains("com//p/"), "{out}");
    }

    #[test]
    fn injected_pixel_survives_the_sanitiser() {
        // The regression this whole module is shaped around: sanitising *after*
        // injecting destroys the pixel twice over — the 1x1 is stripped as a
        // tracking pixel, and anything that did survive would be rewritten to
        // the image proxy and stop being same-origin.
        let sanitized = sanitize_html("<p>Article</p>", SECRET, None, None, None);
        let injected = ctx(Some(hour_ago())).maybe_inject(sanitized, 42, Utc::now());
        assert!(injected.contains("/p/7-42-"), "{injected}");
        assert!(
            !injected.contains("/api/proxy/image"),
            "the pixel must stay same-origin: {injected}"
        );

        let resanitized = sanitize_html(&injected, SECRET, None, None, None);
        assert!(
            !resanitized.contains("/p/7-42-"),
            "sanitising after injection removes the pixel — this is why order matters"
        );
    }
}
