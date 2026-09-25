//! OS notifications posted by the shell, not by the webview.
//!
//! WKWebView (and WebKitGTK) report the page's Notification API denied, and
//! there is no site-settings UI inside the shell to undo that. Desktop apps
//! post through the OS notification service instead — the same path every
//! other app uses. Remote content never gets the plugin surface: one command,
//! a clamped title and body, and a same-origin path opened on click.

use tauri::AppHandle;

const TITLE_CHARS: usize = 180;
const BODY_CHARS: usize = 500;
const TAG_CHARS: usize = 80;
const HREF_BYTES: usize = 2048;

/// A path the shell may `location.assign`. Same-origin only: a leading `/`,
/// no scheme, no protocol-relative `//`, no controls. Anything else is dropped
/// rather than eval'd.
pub fn openable_href(raw: &str) -> Option<&str> {
    let href = raw.trim();
    if href.is_empty()
        || href.len() > HREF_BYTES
        || !href.starts_with('/')
        || href.starts_with("//")
    {
        return None;
    }
    if href.bytes().any(|b| b < 0x20 || b == 0x7f || b == b'\\') {
        return None;
    }
    Some(href)
}

/// `location.assign` of a validated path, JSON-encoded so a quote in the path
/// cannot break out of the string.
pub fn assign_js(raw: &str) -> Option<String> {
    let href = openable_href(raw)?;
    let encoded = serde_json::to_string(href).ok()?;
    Some(format!("window.location.assign({encoded})"))
}

/// Strip controls and cap length. Notification titles and bodies are
/// user- and agent-written; the OS banner is not a place for a raw dump.
pub fn clamp_notice(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    for ch in s.chars() {
        if n >= max_chars {
            break;
        }
        if ch.is_control() {
            continue;
        }
        out.push(ch);
        n += 1;
    }
    out
}

/// Stable non-zero id so a second arrival with the same tag replaces the
/// banner on servers that honor it (Linux). macOS's older center ignores ids;
/// a duplicate there is noise, not a wrong destination.
pub fn tag_id(tag: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for b in tag.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(16_777_619);
    }
    h.max(1)
}

/// Post one banner. `on_open` runs when the person clicks it (the default
/// action), not when they dismiss it. Fails if the OS service refuses the
/// post; the in-app toast already landed, so the caller treats that as soft.
pub fn post(
    app: &AppHandle,
    title: &str,
    body: Option<&str>,
    tag: Option<&str>,
    on_open: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    prepare_bundle(app);
    let mut notification = notify_rust::Notification::new();
    notification.summary(title);
    if let Some(body) = body {
        notification.body(body);
    }
    notification.auto_icon();
    if let Some(tag) = tag {
        notification.id(tag_id(tag));
    }
    // "default" is what a body click invokes on the servers that distinguish
    // a click from a dismiss. Without it, wait_for_action never hears the open.
    notification.action("default", "Open");
    let handle = notification
        .show()
        .map_err(|e| format!("showing the notification: {e}"))?;
    // Blocks until the banner is clicked or dismissed. One thread per visible
    // banner; they exit when the OS retires it.
    tauri::async_runtime::spawn_blocking(move || {
        handle.wait_for_action(|action| {
            if action == "default" {
                on_open();
            }
        });
    });
    Ok(())
}

/// macOS will not show a banner for an unpackaged binary under our bundle id.
/// Dev builds attribute to Terminal, the same workaround the Tauri notification
/// plugin uses; a bundled release uses `app.talaria.desktop`.
#[cfg(target_os = "macos")]
fn prepare_bundle(app: &AppHandle) {
    let id = app.config().identifier.clone();
    let bundle = if tauri::is_dev() {
        "com.apple.Terminal"
    } else {
        id.as_str()
    };
    let _ = notify_rust::set_application(bundle);
}

#[cfg(not(target_os = "macos"))]
fn prepare_bundle(_app: &AppHandle) {}

pub fn clamped_title(raw: &str) -> String {
    clamp_notice(raw, TITLE_CHARS)
}

pub fn clamped_body(raw: &str) -> String {
    clamp_notice(raw, BODY_CHARS)
}

pub fn clamped_tag(raw: &str) -> String {
    clamp_notice(raw, TAG_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openable_href_keeps_a_same_origin_path() {
        assert_eq!(openable_href("/comms?c=1"), Some("/comms?c=1"));
        assert_eq!(openable_href("  /inbox  "), Some("/inbox"));
        assert_eq!(openable_href("/"), Some("/"));
    }

    #[test]
    fn openable_href_rejects_anything_that_is_not_a_path() {
        assert_eq!(openable_href("https://evil.example"), None);
        assert_eq!(openable_href("//evil.example"), None);
        assert_eq!(openable_href("javascript:alert(1)"), None);
        assert_eq!(openable_href("/foo\nbar"), None);
        assert_eq!(openable_href("/foo\\bar"), None);
        assert_eq!(openable_href(""), None);
        assert_eq!(openable_href("   "), None);
    }

    #[test]
    fn assign_js_json_encodes_the_path() {
        let js = assign_js(r#"/comms?c=1&q="hi""#).expect("path");
        assert_eq!(js, r#"window.location.assign("/comms?c=1&q=\"hi\"")"#);
        assert!(assign_js("https://evil.example").is_none());
    }

    #[test]
    fn tag_id_is_stable_and_nonzero() {
        assert_eq!(tag_id("n-1"), tag_id("n-1"));
        assert_ne!(tag_id("n-1"), tag_id("n-2"));
        assert_ne!(tag_id(""), 0);
    }

    #[test]
    fn clamp_notice_drops_controls_and_caps_length() {
        assert_eq!(clamp_notice("a\nb\u{0007}c", 10), "abc");
        assert_eq!(clamp_notice("abcdef", 3), "abc");
    }
}
