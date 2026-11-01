// The one href shape a notification may carry, and what a row stores for a
// writer's href.

use std::sync::{LazyLock, Mutex};

/// The longest href a row keeps — the shell's `openable_href` bound.
const HREF_BYTES: usize = 2048;

/// The ONE href shape a notification may carry: a same-origin path. A leading
/// `/` and not `//` (protocol-relative, another host), no backslash (`/\`
/// is another host to some engines), no control characters, no `.` or `..`
/// path segment (in any `%2e` spelling), at most 2,048 bytes. The same rule
/// the page (`ui/src/lib/notify-open.ts`) and the service worker (`sw.ts`)
/// apply before they open, so a row the server keeps is a row every client
/// will open. Trimmed; `None` for anything else.
///
/// Why the dot segments: a URL parser resolves `/..//evil.example` (or
/// `/./`, `/a/../`, `/%2e%2e/`) to the path `//evil.example` on this origin,
/// and a client that hands that PATH back to a parser — a router, a service
/// worker's `navigate()` — gets the protocol-relative URL of another host.
/// No producer builds an href with a dot segment; refusing them keeps the
/// raw string and the resolved path the same thing.
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
    if has_dot_segment(href) {
        return None;
    }
    Some(href)
}

/// Does the PATH part (before any `?` or `#`) hold a segment a URL parser
/// would resolve away? The WHATWG URL standard's single-dot (`.`, `%2e`) and
/// double-dot (`..`, `.%2e`, `%2e.`, `%2e%2e`) segments, ASCII
/// case-insensitive. A query or fragment is never path-resolved, so a
/// `?next=/../x` stays allowed.
fn has_dot_segment(href: &str) -> bool {
    let path = href.split(['?', '#']).next().unwrap_or("");
    path.split('/').any(|seg| {
        let s = seg.to_ascii_lowercase().replace("%2e", ".");
        s == "." || s == ".."
    })
}

/// Kinds whose writer already handed over an href the rule refused — logged
/// once per kind and process, so a producer bug is visible without flooding.
static HREF_REFUSALS: LazyLock<Mutex<std::collections::HashSet<String>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

/// What a row stores for a writer's href: the validated path, or `''` — a
/// row with no link still lands and still clears on click. Every click path
/// (the bell, a toast, the service worker, the shell) opens only a path, so
/// an absolute or protocol-relative href could only ever have been a way off
/// the instance.
pub(crate) fn stored_href(kind: &str, href: Option<&str>) -> String {
    let raw = href.unwrap_or("");
    if raw.trim().is_empty() {
        return String::new();
    }
    match openable_href(raw) {
        Some(h) => h.to_string(),
        None => {
            let mut seen = HREF_REFUSALS.lock().unwrap_or_else(|p| p.into_inner());
            if seen.insert(kind.to_string()) {
                tracing::warn!(
                    "[notifications] a {kind:?} writer passed an href that is not a same-origin path; stored without a link (logged once per kind)"
                );
            }
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The shell's own cases (desktop/src-tauri/src/notify.rs), so a row the
    // server keeps is a row the shell will open — and nothing else.
    #[test]
    fn openable_href_keeps_paths_and_refuses_everything_else() {
        assert_eq!(openable_href("/comms?c=1"), Some("/comms?c=1"));
        assert_eq!(openable_href("  /inbox  "), Some("/inbox"));
        assert_eq!(openable_href("/"), Some("/"));
        assert_eq!(openable_href("https://evil.example"), None);
        assert_eq!(openable_href("//evil.example"), None);
        assert_eq!(openable_href("/\\evil.example"), None);
        assert_eq!(openable_href("javascript:alert(1)"), None);
        assert_eq!(openable_href("/foo\nbar"), None);
        assert_eq!(openable_href("/foo\\bar"), None);
        assert_eq!(openable_href("/a\u{7f}"), None);
        assert_eq!(openable_href(""), None);
        assert_eq!(openable_href("   "), None);
        let long = format!("/{}", "a".repeat(2047));
        assert_eq!(openable_href(&long), Some(long.as_str()));
        let too_long = format!("/{}", "a".repeat(2048));
        assert_eq!(openable_href(&too_long), None);
    }

    // A dot segment resolves `/..//evil.example` to the PATH `//evil.example`,
    // which a second parse (a router, a worker's navigate) reads as another
    // host. Refused in every spelling; dots that are not a whole segment, and
    // dots in a query or fragment, are ordinary characters. The same table
    // is in ui/src/lib/notify-open.test.ts and ui/src/sw.test.ts.
    #[test]
    fn openable_href_refuses_dot_segments_in_every_spelling() {
        for bad in [
            "/..//evil.example",
            "/.//evil.example",
            "/a/..//evil.example",
            "/%2e%2e//evil.example",
            "/%2E%2e//evil.example",
            "/.%2e//evil.example",
            "/%2e.//evil.example",
            "/%2e//evil.example",
            "/a/b/../../..//evil.example",
            "/boards/..",
            "/boards/.",
            "/./boards",
        ] {
            assert_eq!(openable_href(bad), None, "{bad}");
            assert_eq!(stored_href("mention", Some(bad)), "", "{bad}");
        }
        for good in [
            "/.../x",
            "/..a//x",
            "/a.b/c",
            "/x?next=/..//evil.example",
            "/x#/..//evil.example",
            "/%2e%2ea/x",
        ] {
            assert_eq!(openable_href(good), Some(good), "{good}");
        }
    }

    #[test]
    fn a_refused_href_is_stored_empty_and_a_path_is_kept() {
        assert_eq!(stored_href("mention", Some("https://evil.example")), "");
        assert_eq!(stored_href("mention", Some("//evil.example")), "");
        assert_eq!(stored_href("mention", None), "");
        assert_eq!(stored_href("mention", Some("")), "");
        assert_eq!(stored_href("mention", Some(" /boards/b/t ")), "/boards/b/t");
    }
}
