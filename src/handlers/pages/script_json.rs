//! Helpers for serializing data for safe inline embedding inside HTML
//! `<script>` tags in the SSR shell.

use crate::middleware::flash::FlashMessage;

/// Escape a JSON string for safe embedding inside HTML `<script>` tags.
/// Replaces `</` with `<\/` to prevent `</script>` breakout attacks.
fn escape_json_for_script(json: &str) -> String {
    json.replace("</", "<\\/")
}

/// Serialize an already-built sidebar payload for inline embedding in the
/// shell. Escapes `</` to prevent `</script>` breakout.
pub fn serialize_sidebar_for_script(payload: &crate::handlers::user::SidebarResponse) -> String {
    let json = serde_json::to_string(payload).unwrap_or_else(|_| "null".to_string());
    escape_json_for_script(&json)
}

/// Serialize the pending flash messages for inline embedding in the shell.
pub fn flash_bootstrap_json(messages: &[FlashMessage]) -> String {
    let json = serde_json::to_string(messages).unwrap_or_else(|_| "[]".to_string());
    escape_json_for_script(&json)
}
