//! The open-tracking pixel: a 1x1 image appended to rendered entry content so
//! the server learns which entries a client rendered.
//!
//! **Inject on the output of [`sanitize_html`], never its input**: the
//! sanitiser strips 1x1 images and proxies every `<img>`, which would destroy
//! a same-origin pixel. Pinned by `injected_pixel_survives_the_sanitiser`.
//!
//! [`sanitize_html`]: crate::services::sanitize_html

use chrono::{DateTime, Utc};

use crate::secret;

/// The transparent GIF served for every pixel request, valid or not; inlined so
/// the response depends on nothing the request controls.
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

/// Pixel endpoint prefix: short, outside `/api`, `.gif` so clients treat it as
/// an image. Skipped by `middleware::forward_auth` and `middleware::csrf`: the
/// request carries no session and must not be given one.
pub const PIXEL_PATH_PREFIX: &str = "/p/";

/// Everything needed to decide on, address and sign a pixel. Built once per
/// request, not per entry (a `GReader` page can hold a thousand items).
#[derive(Debug, Clone, Copy)]
pub struct PixelContext<'a> {
    pub user_id: i64,
    /// When this reader opted in, or `None` for opted out.
    pub enabled_at: Option<DateTime<Utc>>,
    /// Root key; the signature derives under [`secret::DOMAIN_PIXEL`].
    pub secret: &'a [u8],
    /// Absolute base for content rendered off-origin (`GReader`); `None` yields a
    /// root-relative URL.
    pub base_url: Option<&'a str>,
}

impl<'a> PixelContext<'a> {
    /// An opted-out context, for paths with no reader settings.
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

    /// Append the pixel when tracking is on and the entry was created after the
    /// opt-in (older entries are outside the denominator); else return `html`.
    ///
    /// # Ordering
    ///
    /// Call on the *result* of `sanitize_html`. See the module docs.
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
        // Empty content stays empty: no article rendered, no open to record.
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
        // Pre-opt-in backlog is outside the denominator.
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
        // Sanitising after injecting would strip the 1x1 and proxy what survived.
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
