// What the pack lets an agent run on Google's Workspace MCP servers, per tool.
//
// Three answers. ALLOW relays the call. HOLD is a change a person approves
// first. DENY is never run for an agent. Before holds are built (the
// `mcp_call` approval, a later change), a HOLD is refused like a DENY:
// no Google write runs without a person, and in this release no person can
// approve one, so none runs.
//
// ALLOW needs BOTH of two facts, and neither is enough alone:
//   · the tool is on the pinned read list below, which names the tools
//     Google's live catalogs marked read-only on 2026-11-17 (fixtures/
//     tools-list/); and
//   · the server's own catalog, as last refreshed, marks it `readOnlyHint:
//     true`.
// Google documents tool annotations as untrusted hints, so a catalog that
// marks a write read-only allows nothing off the list; and a pinned name
// whose annotation changed stops being allowed at the next refresh. A tool
// Google adds during the preview is on no list, so it is held.
//
// DENY is the owner's list (decision D4, 2026-11-16): moving mail to or out
// of the trash, marking or unmarking spam, and applying a sensitive label.
// Talaria's own REST tools refuse TRASH and SPAM too.

use serde::Serialize;
use serde_json::Value;

/// What the pack does with one tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Policy {
    /// Relayed to Google.
    Allow,
    /// A change a person approves first. Refused until holds ship
    /// (`HOLDS_ENABLED`).
    Hold,
    /// Never run for an agent.
    Deny,
}

/// Whether a held call can be queued for a person. It cannot yet: until the
/// `mcp_call` approval ships, the gateway refuses every held call.
pub const HOLDS_ENABLED: bool = false;

/// The tools each product may run without a person: every tool Google's
/// unauthenticated `tools/list` marked `readOnlyHint: true` on 2026-11-17
/// (fixtures/tools-list/). Keyed by `Product::id`.
pub const READ_LIST: &[(&str, &[&str])] = &[
    (
        "gmail",
        &[
            "get_draft",
            "get_message",
            "get_thread",
            "list_drafts",
            "list_labels",
            "search_threads",
        ],
    ),
    (
        "drive",
        &[
            "download_file_content",
            "get_file_metadata",
            "get_file_permissions",
            "list_recent_files",
            "read_file_content",
            "search_files",
        ],
    ),
    (
        "calendar",
        &[
            "get_event",
            "list_calendars",
            "list_events",
            "search_events",
            "suggest_time",
        ],
    ),
    ("docs", &["read_doc"]),
    ("sheets", &["get_spreadsheet", "get_values"]),
    (
        "slides",
        &[
            "read_presentation",
            "read_slide_page",
            "read_slide_page_thumbnail",
        ],
    ),
    (
        "chat",
        &["list_messages", "search_conversations", "search_messages"],
    ),
    (
        "people",
        &[
            "get_user_profile",
            "search_contacts",
            "search_directory_people",
        ],
    ),
    ("search", &["search_corpus"]),
];

/// Is `tool` on `product`'s pinned read list?
pub fn pinned_read(product: &str, tool: &str) -> bool {
    READ_LIST
        .iter()
        .find(|(p, _)| *p == product)
        .is_some_and(|(_, tools)| tools.contains(&tool))
}

/// The owner's deny list (D4), by the words in a tool's name: trash and
/// untrash, spam and unspam, and any `apply_sensitive_*`. Matched on whole
/// `_`-separated words, so a tool Google adds with the same verb is denied
/// too — the strict direction.
pub fn denied_by_name(tool: &str) -> bool {
    let lower = tool.to_ascii_lowercase();
    lower.starts_with("apply_sensitive_")
        || lower
            .split('_')
            .any(|w| matches!(w, "trash" | "untrash" | "spam" | "unspam"))
}

/// Does the catalog entry mark the tool read-only? Only a JSON `true`
/// counts; absent, null or anything else is not read-only.
pub fn marked_read_only(entry: Option<&Value>) -> bool {
    entry
        .and_then(|e| e.pointer("/annotations/readOnlyHint"))
        .and_then(Value::as_bool)
        == Some(true)
}

/// The pack's default for `tool` on `product`, given the tool's entry in the
/// server's cached catalog (None when the catalog does not list it).
pub fn default_policy(product: &str, tool: &str, entry: Option<&Value>) -> Policy {
    if denied_by_name(tool) {
        Policy::Deny
    } else if pinned_read(product, tool) && marked_read_only(entry) {
        Policy::Allow
    } else {
        Policy::Hold
    }
}

/// The entry for `tool` in a cached catalog.
pub fn catalog_entry<'a>(catalog: &'a Value, tool: &str) -> Option<&'a Value> {
    catalog
        .as_array()?
        .iter()
        .find(|t| t.get("name").and_then(Value::as_str) == Some(tool))
}

/// What the gateway does with a call to `tool`: the default, with a hold
/// refused while holds are not built.
pub fn gateway_policy(product: &str, tool: &str, catalog: &Value) -> Policy {
    match default_policy(product, tool, catalog_entry(catalog, tool)) {
        Policy::Hold if !HOLDS_ENABLED => Policy::Deny,
        p => p,
    }
}

/// The tools of a cached catalog an agent may call: those whose default is
/// ALLOW. The read-only subset the gateway lists and relays.
pub fn allowed_tools(product: &str, catalog: &Value) -> Vec<String> {
    catalog
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let name = t.get("name").and_then(Value::as_str)?;
            (default_policy(product, name, Some(t)) == Policy::Allow).then(|| name.to_string())
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::PRODUCTS;
    use crate::catalog::read_tools_list;
    use serde_json::json;

    /// Each product's `tools/list`, captured from Google with no credential
    /// on 2026-11-17 (POST https://<host>/mcp/v1), normalized as the cache
    /// keeps it.
    pub(crate) fn live_catalog(product: &str) -> Value {
        let body = match product {
            "gmail" => include_str!("../fixtures/tools-list/gmailmcp.json"),
            "drive" => include_str!("../fixtures/tools-list/drivemcp.json"),
            "calendar" => include_str!("../fixtures/tools-list/calendarmcp.json"),
            "docs" => include_str!("../fixtures/tools-list/docsmcp.json"),
            "sheets" => include_str!("../fixtures/tools-list/sheetsmcp.json"),
            "slides" => include_str!("../fixtures/tools-list/slidesmcp.json"),
            "chat" => include_str!("../fixtures/tools-list/chatmcp.json"),
            "people" => include_str!("../fixtures/tools-list/people.json"),
            "search" => include_str!("../fixtures/tools-list/workspacemcp.json"),
            other => panic!("no fixture for {other}"),
        };
        Value::Array(read_tools_list(200, body).expect("a captured catalog reads"))
    }

    fn names(catalog: &Value) -> Vec<String> {
        catalog
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn every_product_allows_exactly_its_pinned_reads_on_googles_live_catalog() {
        assert_eq!(READ_LIST.len(), PRODUCTS.len(), "one list per product");
        for p in PRODUCTS {
            let catalog = live_catalog(p.id);
            let mut allowed = allowed_tools(p.id, &catalog);
            allowed.sort();
            let mut pinned: Vec<String> = READ_LIST
                .iter()
                .find(|(id, _)| *id == p.id)
                .unwrap()
                .1
                .iter()
                .map(|s| s.to_string())
                .collect();
            pinned.sort();
            assert_eq!(allowed, pinned, "{}", p.id);
            // Every tool Google marked read-only that day is pinned, and
            // every other tool is held or denied, never allowed.
            for name in names(&catalog) {
                let entry = catalog_entry(&catalog, &name);
                let policy = default_policy(p.id, &name, entry);
                if marked_read_only(entry) {
                    assert_eq!(policy, Policy::Allow, "{}/{name}", p.id);
                } else {
                    assert_ne!(policy, Policy::Allow, "{}/{name}", p.id);
                }
            }
        }
    }

    #[test]
    fn the_owners_deny_list_covers_every_trash_spam_and_sensitive_label_tool() {
        let catalog = live_catalog("gmail");
        let mut denied: Vec<String> = names(&catalog)
            .into_iter()
            .filter(|n| default_policy("gmail", n, catalog_entry(&catalog, n)) == Policy::Deny)
            .collect();
        denied.sort();
        assert_eq!(
            denied,
            [
                "apply_sensitive_message_label",
                "apply_sensitive_thread_label",
                "mark_message_spam",
                "mark_thread_spam",
                "trash_message",
                "trash_thread",
                "unmark_message_spam",
                "unmark_thread_spam",
                "untrash_message",
                "untrash_thread",
            ]
        );
        // Every other Gmail write is held — create_draft included (D4).
        for held in [
            "create_draft",
            "create_label",
            "label_message",
            "label_thread",
            "unlabel_message",
            "unlabel_thread",
            "update_message_labels",
        ] {
            assert_eq!(
                default_policy("gmail", held, catalog_entry(&catalog, held)),
                Policy::Hold,
                "{held}"
            );
        }
        // The same verbs on a tool Google has not shipped are denied too.
        assert!(denied_by_name("trash_file"));
        assert!(denied_by_name("Untrash_File"));
        assert!(!denied_by_name("list_trashed_labels_count"));
        assert!(!denied_by_name("spammy_but_not_a_word"));
    }

    #[test]
    fn annotations_alone_never_allow_and_the_list_alone_never_allows() {
        let ro = json!({ "name": "x", "annotations": { "readOnlyHint": true } });
        // A tool absent from the read list, marked read-only: held.
        assert_eq!(
            default_policy("gmail", "brand_new_read", Some(&ro)),
            Policy::Hold
        );
        // A pinned tool whose annotation is missing, false, null or a string.
        for entry in [
            json!({ "name": "list_labels" }),
            json!({ "name": "list_labels", "annotations": {} }),
            json!({ "name": "list_labels", "annotations": { "readOnlyHint": false } }),
            json!({ "name": "list_labels", "annotations": { "readOnlyHint": null } }),
            json!({ "name": "list_labels", "annotations": { "readOnlyHint": "true" } }),
        ] {
            assert_eq!(
                default_policy("gmail", "list_labels", Some(&entry)),
                Policy::Hold,
                "{entry}"
            );
        }
        // A pinned tool the catalog does not list: held.
        assert_eq!(default_policy("gmail", "list_labels", None), Policy::Hold);
        // A pinned name under another product is not pinned there.
        assert_eq!(
            default_policy("drive", "list_labels", Some(&ro)),
            Policy::Hold
        );
        // A write Google mislabelled read-only is still not on the list.
        let lying = json!([{ "name": "create_event", "annotations": { "readOnlyHint": true } }]);
        assert!(allowed_tools("calendar", &lying).is_empty());
    }

    #[test]
    fn before_holds_ship_the_gateway_refuses_every_held_call() {
        const { assert!(!HOLDS_ENABLED) };
        let catalog = live_catalog("calendar");
        assert_eq!(
            gateway_policy("calendar", "list_events", &catalog),
            Policy::Allow
        );
        for write in [
            "create_event",
            "update_event",
            "delete_event",
            "respond_to_event",
        ] {
            assert_eq!(
                gateway_policy("calendar", write, &catalog),
                Policy::Deny,
                "{write}"
            );
        }
        assert_eq!(
            gateway_policy("calendar", "not_listed", &catalog),
            Policy::Deny
        );
        assert_eq!(
            gateway_policy("calendar", "list_events", &json!([])),
            Policy::Deny
        );
    }
}
