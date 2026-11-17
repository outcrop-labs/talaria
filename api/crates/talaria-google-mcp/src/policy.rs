// What the pack lets an agent run on Google's Workspace MCP servers, per tool.
//
// Three answers. ALLOW relays the call. HOLD queues it for a person, who
// approves or rejects that exact call (the `mcp_call` approval in
// talaria-google-pending); nothing reaches Google's MCP servers until they
// approve. DENY is never run for an agent.
//
// THE DEFAULT. ALLOW needs BOTH of two facts, and neither is enough alone:
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
//
// THE SAME FLOOR, BY ARGUMENT. Gmail's label tools reach the trash and spam
// without the words in their names: Google's own catalog says `unlabel_thread`
// and `unlabel_message` take "a system label ID (e.g., `INBOX`, `TRASH`, …)",
// and `label_*` and `update_message_labels` take system label IDs too. So a
// call of ANY tool that names TRASH, SPAM or BIN in a label argument
// (`forbidden_label`) is refused at the gateway, before a hold, an allow or
// an unlock is read, and again at the approved replay — no override, no
// unlock and no approval lifts it. The set is the REST organize path's own
// (talaria-google-gmail's `FORBIDDEN_LABELS`).
//
// AN ADMIN'S OVERRIDE (`mcp_tool_policies`, one row per server and tool)
// wins over the default — with one floor: a tool on the deny list stays
// denied whatever the row says (`override_refusal` refuses to write one, and
// `policy_for` ignores one that got there anyway). An override names one
// tool by name on one server, so a tool Google adds later never inherits
// one: it starts at the default, held.

use serde::Serialize;
use serde_json::Value;

/// What the pack does with one tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Policy {
    /// Relayed to Google.
    Allow,
    /// Queued for a person, who approves that exact call before it runs.
    Hold,
    /// Never run for an agent.
    Deny,
}

impl Policy {
    /// The stored and wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Policy::Allow => "allow",
            Policy::Hold => "hold",
            Policy::Deny => "deny",
        }
    }

    /// `allow` | `hold` | `deny`, exactly; anything else is None.
    pub fn parse(raw: &str) -> Option<Policy> {
        match raw {
            "allow" => Some(Policy::Allow),
            "hold" => Some(Policy::Hold),
            "deny" => Some(Policy::Deny),
            _ => None,
        }
    }
}

/// An admin's per-tool overrides for one server: (tool, policy) pairs, as
/// `mcp_tool_policies` stores them.
pub type Overrides = [(String, Policy)];

/// The override an admin set for `tool`, if any.
pub fn override_of(overrides: &Overrides, tool: &str) -> Option<Policy> {
    overrides.iter().find(|(t, _)| t == tool).map(|(_, p)| *p)
}

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

/// The labels no agent call may name, on either side of a change: the REST
/// organize path's set (talaria-google-gmail's `FORBIDDEN_LABELS`). Moving
/// mail to or out of the trash or spam is not organizing.
pub const FORBIDDEN_LABELS: [&str; 3] = ["TRASH", "SPAM", "BIN"];

/// The forbidden label a call's arguments name, as written, or None: any
/// string — alone, or in an array — under any object key whose name
/// contains `label` (`labelIds`, `addLabelIds`, `removeLabelIds`,
/// `labelOption`, …), at any depth, equal to TRASH, SPAM or BIN ignoring
/// case. Read for every tool on every product, whatever its policy: a tool
/// Google adds later with the same arguments is covered too — the strict
/// direction. No read tool on Google's live catalogs takes a label argument.
pub fn forbidden_label(arguments: &Value) -> Option<String> {
    fn labels(v: &Value) -> Vec<&str> {
        match v {
            Value::String(s) => vec![s.as_str()],
            Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        }
    }
    match arguments {
        Value::Object(map) => map.iter().find_map(|(k, v)| {
            k.to_ascii_lowercase()
                .contains("label")
                .then(|| {
                    labels(v).into_iter().find(|l| {
                        FORBIDDEN_LABELS
                            .iter()
                            .any(|f| f.eq_ignore_ascii_case(l.trim()))
                    })
                })
                .flatten()
                .map(str::to_string)
                .or_else(|| forbidden_label(v))
        }),
        Value::Array(items) => items.iter().find_map(forbidden_label),
        _ => None,
    }
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

/// The pack's policy for `tool`: an admin's override when there is one,
/// else the default — and DENY for any tool on the owner's deny list, whatever
/// the override says (the floor no row lifts).
pub fn policy_for(
    product: &str,
    tool: &str,
    entry: Option<&Value>,
    override_: Option<Policy>,
) -> Policy {
    if denied_by_name(tool) {
        return Policy::Deny;
    }
    override_.unwrap_or_else(|| default_policy(product, tool, entry))
}

/// The admin's overrides as they apply to one caller. For a PERSONAL
/// assistant (an owner principal: the call writes into one member's own
/// Google account) an admin's ALLOW is dropped, so a write there stays held
/// for that member — the approval model gives the owner alone, never an
/// admin, the say over an owner's row, and an ALLOW would be an admin's
/// standing approval of every such call. HOLD and DENY rows apply to every
/// caller (the strict direction), and an ALLOW applies to the org account
/// and to agent identities, whose calls an admin approves anyway.
pub fn overrides_for(overrides: &Overrides, personal: bool) -> Vec<(String, Policy)> {
    overrides
        .iter()
        .filter(|(_, p)| !(personal && *p == Policy::Allow))
        .cloned()
        .collect()
}

/// Why an admin may not set `policy` on `tool`, or None when they may: a
/// tool on the owner's deny list takes no override but DENY.
pub fn override_refusal(tool: &str, policy: Policy) -> Option<&'static str> {
    (denied_by_name(tool) && policy != Policy::Deny).then_some(
        "This tool is on the owner's deny list (trash, untrash, spam, unspam and sensitive labels): it is never run for an agent, and no override changes that.",
    )
}

/// What the gateway does with a call to `tool` on a server whose cached
/// catalog is `catalog`, under the admin's `overrides`.
pub fn gateway_policy(product: &str, tool: &str, catalog: &Value, overrides: &Overrides) -> Policy {
    policy_for(
        product,
        tool,
        catalog_entry(catalog, tool),
        override_of(overrides, tool),
    )
}

/// The cached catalog's tools with `want` as their policy, in catalog order.
fn tools_with(
    product: &str,
    catalog: &Value,
    overrides: &Overrides,
    want: impl Fn(Policy) -> bool,
) -> Vec<String> {
    catalog
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let name = t.get("name").and_then(Value::as_str)?;
            want(policy_for(
                product,
                name,
                Some(t),
                override_of(overrides, name),
            ))
            .then(|| name.to_string())
        })
        .collect()
}

/// The tools of a cached catalog an agent may call without a person: those
/// whose DEFAULT is ALLOW (no override read). The pinned reads Google marks
/// read-only.
pub fn allowed_tools(product: &str, catalog: &Value) -> Vec<String> {
    tools_with(product, catalog, &[], |p| p == Policy::Allow)
}

/// The tools an agent is shown and may call: every cached tool that is not
/// denied — the allowed ones run, the held ones queue for a person. A tool
/// the catalog does not list is in neither.
pub fn listed_tools(product: &str, catalog: &Value, overrides: &Overrides) -> Vec<String> {
    tools_with(product, catalog, overrides, |p| p != Policy::Deny)
}

/// The listed tools whose calls are held for a person.
pub fn held_tools(product: &str, catalog: &Value, overrides: &Overrides) -> Vec<String> {
    tools_with(product, catalog, overrides, |p| p == Policy::Hold)
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
    fn a_held_call_is_held_and_a_denied_one_stays_denied() {
        let catalog = live_catalog("calendar");
        assert_eq!(
            gateway_policy("calendar", "list_events", &catalog, &[]),
            Policy::Allow
        );
        for write in [
            "create_event",
            "update_event",
            "delete_event",
            "respond_to_event",
        ] {
            assert_eq!(
                gateway_policy("calendar", write, &catalog, &[]),
                Policy::Hold,
                "{write}"
            );
        }
        // A pinned read the catalog does not list (an empty catalog) is not
        // allowed; the gateway refuses a tool absent from the catalog before
        // it asks the policy.
        assert_eq!(
            gateway_policy("calendar", "list_events", &json!([]), &[]),
            Policy::Hold
        );
        let gmail = live_catalog("gmail");
        for denied in [
            "trash_thread",
            "mark_message_spam",
            "apply_sensitive_thread_label",
        ] {
            assert_eq!(gateway_policy("gmail", denied, &gmail, &[]), Policy::Deny);
        }
    }

    /// Every tool of every live catalog lands where the owner's decisions
    /// put it: reads allowed, the deny list denied, every other tool held —
    /// every case, not a sample.
    #[test]
    fn every_live_tool_is_allowed_held_or_denied_as_decided() {
        for p in PRODUCTS {
            let catalog = live_catalog(p.id);
            let listed = listed_tools(p.id, &catalog, &[]);
            let held = held_tools(p.id, &catalog, &[]);
            for name in names(&catalog) {
                let policy = gateway_policy(p.id, &name, &catalog, &[]);
                let expect = if denied_by_name(&name) {
                    Policy::Deny
                } else if pinned_read(p.id, &name)
                    && marked_read_only(catalog_entry(&catalog, &name))
                {
                    Policy::Allow
                } else {
                    Policy::Hold
                };
                assert_eq!(policy, expect, "{}/{name}", p.id);
                assert_eq!(
                    listed.contains(&name),
                    policy != Policy::Deny,
                    "{}/{name}",
                    p.id
                );
                assert_eq!(
                    held.contains(&name),
                    policy == Policy::Hold,
                    "{}/{name}",
                    p.id
                );
            }
        }
    }

    #[test]
    fn an_override_wins_except_over_the_deny_list() {
        let gmail = live_catalog("gmail");
        let overrides = vec![
            ("create_draft".to_string(), Policy::Allow),
            ("search_threads".to_string(), Policy::Hold),
            ("list_labels".to_string(), Policy::Deny),
            // A row that should never have been written: the floor holds.
            ("trash_thread".to_string(), Policy::Allow),
            ("untrash_message".to_string(), Policy::Hold),
        ];
        assert_eq!(
            gateway_policy("gmail", "create_draft", &gmail, &overrides),
            Policy::Allow
        );
        assert_eq!(
            gateway_policy("gmail", "search_threads", &gmail, &overrides),
            Policy::Hold
        );
        assert_eq!(
            gateway_policy("gmail", "list_labels", &gmail, &overrides),
            Policy::Deny
        );
        assert_eq!(
            gateway_policy("gmail", "trash_thread", &gmail, &overrides),
            Policy::Deny
        );
        assert_eq!(
            gateway_policy("gmail", "untrash_message", &gmail, &overrides),
            Policy::Deny
        );
        // A tool with no row keeps its default.
        assert_eq!(
            gateway_policy("gmail", "label_thread", &gmail, &overrides),
            Policy::Hold
        );
        let listed = listed_tools("gmail", &gmail, &overrides);
        assert!(listed.contains(&"create_draft".to_string()));
        assert!(!listed.contains(&"list_labels".to_string()));
        assert!(!listed.contains(&"trash_thread".to_string()));
        // The default-only reading ignores the rows.
        assert!(allowed_tools("gmail", &gmail).contains(&"list_labels".to_string()));
        assert!(!allowed_tools("gmail", &gmail).contains(&"create_draft".to_string()));
    }

    #[test]
    fn an_admin_may_not_lift_the_deny_list() {
        for tool in [
            "trash_thread",
            "untrash_message",
            "mark_thread_spam",
            "unmark_message_spam",
            "apply_sensitive_message_label",
            "trash_file",
        ] {
            assert!(override_refusal(tool, Policy::Allow).is_some(), "{tool}");
            assert!(override_refusal(tool, Policy::Hold).is_some(), "{tool}");
            assert!(override_refusal(tool, Policy::Deny).is_none(), "{tool}");
        }
        for p in [Policy::Allow, Policy::Hold, Policy::Deny] {
            assert!(override_refusal("create_event", p).is_none());
        }
    }

    #[test]
    fn a_tool_google_adds_later_starts_held_and_inherits_no_override() {
        // A catalog refresh brought a tool nobody pinned, marked read-only.
        let mut tools = live_catalog("calendar").as_array().unwrap().clone();
        tools.push(json!({ "name": "brand_new_tool", "annotations": { "readOnlyHint": true } }));
        let catalog = Value::Array(tools);
        // Every other tool on the server carries an ALLOW override: the new
        // one has its own name, so none of them reach it.
        let overrides: Vec<(String, Policy)> = names(&catalog)
            .into_iter()
            .filter(|n| n != "brand_new_tool")
            .map(|n| (n, Policy::Allow))
            .collect();
        assert_eq!(
            gateway_policy("calendar", "brand_new_tool", &catalog, &overrides),
            Policy::Hold
        );
        assert!(
            held_tools("calendar", &catalog, &overrides).contains(&"brand_new_tool".to_string())
        );
    }

    /// Google's own catalog documents the label tools taking system label
    /// IDs, TRASH among them (`unlabel_*`), so the deny list's words alone
    /// would not keep mail out of the trash or spam: the arguments are read.
    #[test]
    fn a_trash_or_spam_label_in_any_label_argument_is_named() {
        let gmail = live_catalog("gmail");
        let described = |tool: &str| {
            catalog_entry(&gmail, tool).unwrap()["inputSchema"]["properties"]["labelIds"]
                ["description"]
                .as_str()
                .unwrap()
                .to_string()
        };
        // The captured catalog says so in Google's words.
        assert!(described("unlabel_thread").contains("`TRASH`"));
        assert!(described("unlabel_message").contains("`TRASH`"));
        assert!(described("label_thread").contains("system label ID"));
        for args in [
            json!({ "threadId": "t1", "labelIds": ["TRASH"] }),
            json!({ "threadId": "t1", "labelIds": ["UNREAD", "spam"] }),
            json!({ "messageId": "m1", "labelIds": [" Trash "] }),
            json!({ "messageId": "m1", "addLabelIds": ["SPAM"] }),
            json!({ "messageId": "m1", "removeLabelIds": ["INBOX", "TRASH"] }),
            json!({ "messageId": "m1", "labelOption": "TRASH" }),
            json!({ "messageId": "m1", "labelIds": ["BIN"] }),
            // A shape Google has not shipped: nested, singular.
            json!({ "changes": [{ "labelId": "SPAM" }] }),
        ] {
            assert!(forbidden_label(&args).is_some(), "{args}");
        }
        assert_eq!(
            forbidden_label(&json!({ "labelIds": ["INBOX", "TRASH"] })).as_deref(),
            Some("TRASH")
        );
        for args in [
            json!({ "threadId": "t1", "labelIds": ["UNREAD", "STARRED", "Label_12"] }),
            json!({ "messageId": "m1", "addLabelIds": [], "removeLabelIds": ["INBOX"] }),
            // The words outside a label argument are not a label.
            json!({ "subject": "TRASH", "body": "SPAM" }),
            json!({ "query": "in:trash" }),
            json!({ "displayName": "Spam reports" }),
            json!({}),
            Value::Null,
        ] {
            assert_eq!(forbidden_label(&args), None, "{args}");
        }
        // No read tool Google listed takes a label argument at all, so the
        // floor never refuses a read.
        for p in PRODUCTS {
            let catalog = live_catalog(p.id);
            for name in allowed_tools(p.id, &catalog) {
                let props = &catalog_entry(&catalog, &name).unwrap()["inputSchema"]["properties"];
                let keys: Vec<&String> = props
                    .as_object()
                    .map(|o| o.keys().collect())
                    .unwrap_or_default();
                assert!(
                    !keys
                        .iter()
                        .any(|k| k.to_ascii_lowercase().contains("label")),
                    "{}/{name}: {keys:?}",
                    p.id
                );
            }
        }
    }

    #[test]
    fn an_admins_allow_never_reaches_a_personal_assistant() {
        let gmail = live_catalog("gmail");
        let overrides = vec![
            ("create_draft".to_string(), Policy::Allow),
            ("label_thread".to_string(), Policy::Allow),
            ("search_threads".to_string(), Policy::Hold),
            ("list_labels".to_string(), Policy::Deny),
        ];
        let personal = overrides_for(&overrides, true);
        let shared = overrides_for(&overrides, false);
        assert_eq!(shared, overrides, "org account and agent identities");
        // A personal assistant's writes stay held for its owner; the
        // stricter rows still apply to it.
        for (tool, want) in [
            ("create_draft", Policy::Hold),
            ("label_thread", Policy::Hold),
            ("search_threads", Policy::Hold),
            ("list_labels", Policy::Deny),
            ("get_thread", Policy::Allow),
        ] {
            assert_eq!(
                gateway_policy("gmail", tool, &gmail, &personal),
                want,
                "{tool}"
            );
        }
        assert_eq!(
            gateway_policy("gmail", "create_draft", &gmail, &shared),
            Policy::Allow
        );
        // An ALLOW on a pinned read is the default anyway.
        let read_allowed = vec![("get_thread".to_string(), Policy::Allow)];
        assert_eq!(
            gateway_policy(
                "gmail",
                "get_thread",
                &gmail,
                &overrides_for(&read_allowed, true)
            ),
            Policy::Allow
        );
    }

    #[test]
    fn the_stored_spelling_round_trips() {
        for p in [Policy::Allow, Policy::Hold, Policy::Deny] {
            assert_eq!(Policy::parse(p.as_str()), Some(p));
            assert_eq!(serde_json::to_value(p).unwrap(), json!(p.as_str()));
        }
        assert_eq!(Policy::parse("Allow"), None);
        assert_eq!(Policy::parse(""), None);
    }
}
