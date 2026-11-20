// A pack row's catalog, asked of Google: `tools/list` on the product's
// endpoint, then each tool's protected-resource document for the scopes
// Google accepts for it. Scopes come from Google, not from us.
//
// What the cache keeps per tool: `name`, `title`, `description`,
// `inputSchema` and `annotations` as Google listed them, plus
// `requiredScopesAnyOf` — the document's `scopes_supported`, which Google's
// tool reference pages call "Requires one of" — when the document answered.
// `outputSchema` is dropped: nothing reads it, and it is most of the bytes.
//
// The servers are stateless: `tools/list` needs no `initialize` (probed
// 2026-11-12), and answers without a credential. A call carries the bearer
// when the caller has one; Google's answer to an authenticated listing may
// differ from the unauthenticated one, which nobody here could observe yet.

use serde_json::{Map, Value, json};
use talaria_mcp_jsonrpc::parse_mcp_response;
use talaria_safe_fetch::{SafeError, SafeFetch, safe_fetch};

use crate::Product;
use crate::pack::{endpoint, resource_metadata_url};

/// The whole-request budget for one catalog call.
const CATALOG_TIMEOUT_MS: u64 = 30_000;
/// The most a `tools/list` answer may run to (Calendar's was 119 kB).
const CATALOG_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// The most a protected-resource document may run to.
const METADATA_MAX_BYTES: u64 = 64 * 1024;

/// Why no catalog came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    /// The request never got Google's answer.
    Unreachable(String),
    /// Google answered with a refusal: the HTTP status, and Google's words.
    Refused { status: u16, text: Option<String> },
    /// Google answered 2xx with no tool list in it.
    Malformed(String),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogError::Unreachable(e) => f.write_str(e),
            CatalogError::Refused {
                status,
                text: Some(t),
            } => write!(f, "Google answered HTTP {status}: \u{ab}{t}\u{bb}"),
            CatalogError::Refused { status, text: None } => {
                write!(f, "Google answered HTTP {status}")
            }
            CatalogError::Malformed(e) => f.write_str(e),
        }
    }
}

fn unreachable(what: &str, e: SafeError) -> String {
    match e {
        SafeError::Timeout => format!("{what} did not answer in time"),
        SafeError::Blocked(b) => format!("{what} was refused before sending: {b}"),
        SafeError::Fetch(_) => format!("{what} could not be reached"),
    }
}

/// Ask `p`'s server for its catalog, with `bearer` when the caller has one,
/// and each tool's scopes. Redirects are not followed: the bearer goes to
/// the product's endpoint and nowhere else.
pub async fn fetch_catalog(p: &Product, bearer: Option<&str>) -> Result<Vec<Value>, CatalogError> {
    let url = endpoint(p);
    let auth = bearer.map(|b| format!("Bearer {b}"));
    let mut headers = vec![
        ("content-type", "application/json"),
        ("accept", "application/json, text/event-stream"),
    ];
    if let Some(a) = auth.as_deref() {
        headers.push(("authorization", a));
    }
    let body =
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }).to_string();
    let res = safe_fetch(
        &url,
        SafeFetch {
            method: Some("POST"),
            headers,
            body: Some(body.as_bytes()),
            timeout_ms: Some(CATALOG_TIMEOUT_MS),
            max_bytes: Some(CATALOG_MAX_BYTES),
            max_redirects: Some(0),
        },
    )
    .await
    .map_err(|e| {
        CatalogError::Unreachable(unreachable(&format!("Google's {} MCP server", p.name), e))
    })?;
    let text = String::from_utf8_lossy(&res.body);
    let listed = read_tools_list(res.status, &text)?;
    let scopes = futures_util::future::join_all(
        listed
            .iter()
            .map(|t| tool_scopes(p, t.get("name").and_then(Value::as_str).unwrap_or(""))),
    )
    .await;
    Ok(listed
        .into_iter()
        .zip(scopes)
        .map(|(mut tool, scopes)| {
            if let (Some(obj), Some(scopes)) = (tool.as_object_mut(), scopes) {
                obj.insert(
                    "requiredScopesAnyOf".into(),
                    Value::Array(scopes.into_iter().map(Value::String).collect()),
                );
            }
            tool
        })
        .collect())
}

/// Google's answer to `tools/list`, read: the listed tools, normalized
/// (`cache_entry`), or why there are none. A 2xx without a JSON-RPC result
/// carrying a `tools` array is no catalog; a refusal quotes Google.
pub fn read_tools_list(status: u16, body: &str) -> Result<Vec<Value>, CatalogError> {
    if !(200..300).contains(&status) {
        return Err(CatalogError::Refused {
            status,
            text: crate::read_check_answer(status, body).text,
        });
    }
    let Some(rpc) = parse_mcp_response(body) else {
        return Err(CatalogError::Malformed(
            "Google's answer to tools/list was not JSON-RPC".into(),
        ));
    };
    if rpc.get("error").is_some() {
        return Err(CatalogError::Refused {
            status,
            text: crate::read_check_answer(status, body).text,
        });
    }
    let Some(tools) = rpc.pointer("/result/tools").and_then(Value::as_array) else {
        return Err(CatalogError::Malformed(
            "Google's answer to tools/list carried no tool list".into(),
        ));
    };
    Ok(tools.iter().filter_map(cache_entry).collect())
}

/// One listed tool as the cache keeps it, or None for an entry without a
/// string name.
pub fn cache_entry(tool: &Value) -> Option<Value> {
    let obj = tool.as_object()?;
    let name = obj.get("name").and_then(Value::as_str)?;
    if name.is_empty() {
        return None;
    }
    let mut e = Map::new();
    e.insert("name".into(), Value::String(name.to_string()));
    for key in ["title", "description", "inputSchema", "annotations"] {
        if let Some(v) = obj.get(key).filter(|v| !v.is_null()) {
            e.insert(key.into(), v.clone());
        }
    }
    Some(Value::Object(e))
}

/// The scopes Google accepts for `tool`, from its protected-resource
/// document; None when the document did not answer with a list (the tool's
/// scopes are then unknown, and Google decides at call time).
async fn tool_scopes(p: &Product, tool: &str) -> Option<Vec<String>> {
    if tool.is_empty() || !tool.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let res = safe_fetch(
        &resource_metadata_url(p, tool),
        SafeFetch {
            method: Some("GET"),
            headers: vec![("accept", "application/json")],
            timeout_ms: Some(CATALOG_TIMEOUT_MS),
            max_bytes: Some(METADATA_MAX_BYTES),
            max_redirects: Some(0),
            ..SafeFetch::default()
        },
    )
    .await
    .ok()?;
    if !(200..300).contains(&res.status) {
        return None;
    }
    let doc: Value = serde_json::from_slice(&res.body).ok()?;
    scopes_supported(&doc)
}

/// A protected-resource document's `scopes_supported` (RFC 9728), or None
/// when it carries none.
pub fn scopes_supported(doc: &Value) -> Option<Vec<String>> {
    let scopes: Vec<String> = doc
        .get("scopes_supported")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    (!scopes.is_empty()).then_some(scopes)
}

/// The prefix Google's scope URLs share; a grant's stored scope may spell a
/// scope either way.
const SCOPE_PREFIX: &str = "https://www.googleapis.com/auth/";

fn scope_key(s: &str) -> &str {
    s.strip_prefix(SCOPE_PREFIX).unwrap_or(s)
}

/// A grant that cannot call a tool: the scopes Google accepts for it, none
/// of which the grant holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingScope {
    pub tool: String,
    pub any_of: Vec<String>,
}

/// Does a grant whose stored scope is `granted` (space-joined, as Google's
/// token answer lists it) hold one of the scopes `tool` accepts? None when
/// it does — and when either side is unknown (no scopes cached for the tool,
/// or nothing stored for the grant): Google decides then, and its answer is
/// relayed.
pub fn missing_scope(
    granted: Option<&str>,
    tool: &str,
    entry: Option<&Value>,
) -> Option<MissingScope> {
    let granted: Vec<&str> = granted?.split_whitespace().map(scope_key).collect();
    if granted.is_empty() {
        return None;
    }
    let any_of: Vec<String> = entry?
        .get("requiredScopesAnyOf")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    if any_of.is_empty() || any_of.iter().any(|s| granted.contains(&scope_key(s))) {
        return None;
    }
    Some(MissingScope {
        tool: tool.to_string(),
        any_of,
    })
}

/// The sentence for a missing scope. `requestable` is the scopes the
/// principal's connect flow asks Google for: when one of them is accepted, a
/// reconnect grants it; when none is, reconnecting cannot help.
pub fn missing_scope_sentence(
    p: &Product,
    m: &MissingScope,
    requestable: &[&str],
    fix: &str,
) -> String {
    let accepted = m
        .any_of
        .iter()
        .map(|s| scope_key(s).to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let grantable = m
        .any_of
        .iter()
        .find(|s| requestable.iter().any(|r| scope_key(r) == scope_key(s)));
    let lead = format!(
        "Google's {} MCP server runs \u{201c}{}\u{201d} only with one of these scopes: {accepted}. This Google connection was granted none of them. Nothing was sent to Google's Workspace MCP servers.",
        p.name, m.tool
    );
    match grantable {
        Some(s) => format!(
            "{lead} Reconnect to grant {} (needed by {}): {fix}",
            scope_key(s),
            m.tool
        ),
        None => format!(
            "{lead} Talaria's Google connection does not ask Google for any of them, so reconnecting would not help."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GMAIL, PRODUCTS, pack};

    /// Every tool's protected-resource document, captured from Google with no
    /// credential on 2026-11-17 (GET https://<host>/.well-known/
    /// oauth-protected-resource/<tool>, each HTTP 200), keyed by that URL
    /// without its scheme.
    fn documents() -> Value {
        let all: Value =
            serde_json::from_str(include_str!("../fixtures/protected-resource.json")).unwrap();
        all["documents"].clone()
    }

    fn host(p: &Product) -> &'static str {
        p.endpoint
            .trim_start_matches("https://")
            .split('/')
            .next()
            .unwrap()
    }

    /// A product's live catalog with the scopes its documents list, as
    /// `fetch_catalog` would cache it.
    fn cached(p: &Product) -> Value {
        let docs = documents();
        let mut catalog = crate::policy::tests::live_catalog(p.id);
        for t in catalog.as_array_mut().unwrap() {
            let name = t["name"].as_str().unwrap().to_string();
            let doc = &docs[format!("{}/.well-known/oauth-protected-resource/{name}", host(p))];
            if let Some(scopes) = scopes_supported(doc) {
                t["requiredScopesAnyOf"] = json!(scopes);
            }
        }
        catalog
    }

    /// The personal connect's scopes (talaria-google-oauth WORKSPACE_SCOPES,
    /// as Google's token answer lists them).
    const PERSONAL: &str = "openid email https://www.googleapis.com/auth/drive https://www.googleapis.com/auth/calendar.events https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.send";
    /// The org connect's (ORG_CONNECT_SCOPES).
    const ORG: &str = "openid email https://www.googleapis.com/auth/calendar https://www.googleapis.com/auth/drive https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.send";

    #[test]
    fn every_listed_tool_has_googles_scopes_and_the_cache_keeps_what_policy_reads() {
        let docs = documents();
        for p in PRODUCTS {
            for t in cached(&p).as_array().unwrap() {
                let name = t["name"].as_str().unwrap();
                assert!(
                    t["requiredScopesAnyOf"]
                        .as_array()
                        .is_some_and(|a| !a.is_empty()),
                    "{}/{name} has a document listing scopes",
                    p.id
                );
                assert!(t.get("annotations").is_some(), "{}/{name}", p.id);
                assert!(t.get("inputSchema").is_some(), "{}/{name}", p.id);
                assert!(t.get("outputSchema").is_none(), "{}/{name}", p.id);
            }
            // The server-wide document lists every scope a tool takes.
            let all = scopes_supported(
                &docs[format!("{}/.well-known/oauth-protected-resource/mcp/v1", host(&p))],
            )
            .unwrap();
            assert!(!all.is_empty(), "{}", p.id);
        }
        // Google's own spelling, verbatim.
        assert_eq!(
            scopes_supported(
                &docs["gmailmcp.googleapis.com/.well-known/oauth-protected-resource/list_labels"]
            ),
            Some(vec![
                "https://mail.google.com/".to_string(),
                "https://www.googleapis.com/auth/gmail.labels".to_string(),
                "https://www.googleapis.com/auth/gmail.labels.readonly".to_string(),
                "https://www.googleapis.com/auth/gmail.metadata".to_string(),
                "https://www.googleapis.com/auth/gmail.modify".to_string(),
                "https://www.googleapis.com/auth/gmail.readonly".to_string(),
            ])
        );
    }

    /// Which pinned reads each connect's grant can run, per Google's
    /// documents — every case, not a sample.
    #[test]
    fn scope_selection_for_every_pinned_read_under_each_connect() {
        let mut short_personal = Vec::new();
        let mut short_org = Vec::new();
        for p in PRODUCTS {
            let catalog = cached(&p);
            for name in crate::policy::allowed_tools(p.id, &catalog) {
                let entry = crate::policy::catalog_entry(&catalog, &name);
                if missing_scope(Some(PERSONAL), &name, entry).is_some() {
                    short_personal.push(format!("{}/{name}", p.id));
                }
                if missing_scope(Some(ORG), &name, entry).is_some() {
                    short_org.push(format!("{}/{name}", p.id));
                }
            }
        }
        short_personal.sort();
        short_org.sort();
        // calendar.events does not cover list_calendars; Chat and People
        // take scopes no Talaria connect asks for.
        assert_eq!(
            short_personal,
            [
                "calendar/list_calendars",
                "chat/list_messages",
                "chat/search_conversations",
                "chat/search_messages",
                "people/get_user_profile",
                "people/search_contacts",
                "people/search_directory_people",
            ]
        );
        assert_eq!(
            short_org,
            [
                "chat/list_messages",
                "chat/search_conversations",
                "chat/search_messages",
                "people/get_user_profile",
                "people/search_contacts",
                "people/search_directory_people",
            ]
        );
    }

    #[test]
    fn an_unknown_side_is_never_a_shortfall() {
        let entry = json!({ "name": "list_labels", "requiredScopesAnyOf": ["https://www.googleapis.com/auth/gmail.readonly"] });
        assert!(missing_scope(None, "list_labels", Some(&entry)).is_none());
        assert!(missing_scope(Some("  "), "list_labels", Some(&entry)).is_none());
        assert!(missing_scope(Some("openid"), "list_labels", None).is_none());
        assert!(
            missing_scope(
                Some("openid"),
                "list_labels",
                Some(&json!({ "name": "list_labels" }))
            )
            .is_none()
        );
        assert!(
            missing_scope(
                Some("openid"),
                "list_labels",
                Some(&json!({ "name": "list_labels", "requiredScopesAnyOf": [] }))
            )
            .is_none()
        );
        // The short spelling of a granted scope counts.
        assert!(missing_scope(Some("gmail.readonly"), "list_labels", Some(&entry)).is_none());
        let short = missing_scope(Some("openid email"), "list_labels", Some(&entry)).unwrap();
        assert_eq!(
            short.any_of,
            ["https://www.googleapis.com/auth/gmail.readonly"]
        );
    }

    #[test]
    fn the_missing_scope_sentence_says_whether_a_reconnect_helps() {
        let m = MissingScope {
            tool: "list_calendars".into(),
            any_of: vec![
                "https://www.googleapis.com/auth/calendar".into(),
                "https://www.googleapis.com/auth/calendar.readonly".into(),
            ],
        };
        let cal = pack::product("calendar").unwrap();
        let helps = missing_scope_sentence(
            &cal,
            &m,
            &["https://www.googleapis.com/auth/calendar"],
            "an admin reconnects the org account under Admin → Org.",
        );
        assert!(
            helps.contains("Reconnect to grant calendar (needed by list_calendars)"),
            "{helps}"
        );
        assert!(helps.contains("Nothing was sent to Google's Workspace MCP servers."));
        let hopeless = missing_scope_sentence(&cal, &m, &["openid"], "unused");
        assert!(
            hopeless.ends_with("so reconnecting would not help."),
            "{hopeless}"
        );
    }

    #[test]
    fn a_refused_or_malformed_listing_is_no_catalog() {
        assert_eq!(
            read_tools_list(401, include_str!("../fixtures/list-labels-401-nocred.json")),
            Err(CatalogError::Refused {
                status: 401,
                text: Some("Request is missing required authentication credential. Expected OAuth 2 access token, login cookie or other valid authentication credential. See https://developers.google.com/identity/sign-in/web/devconsole-project.".into()),
            })
        );
        assert!(matches!(
            read_tools_list(200, include_str!("../fixtures/unknown-method-200.json")),
            Err(CatalogError::Refused { status: 200, text: Some(t) }) if t == "Method not supported"
        ));
        assert!(matches!(
            read_tools_list(200, include_str!("../fixtures/initialize-200.json")),
            Err(CatalogError::Malformed(_))
        ));
        assert!(matches!(
            read_tools_list(404, include_str!("../fixtures/wrong-path-404.html")),
            Err(CatalogError::Refused { status: 404, .. })
        ));
        assert!(matches!(
            read_tools_list(200, "<html>"),
            Err(CatalogError::Malformed(_))
        ));
        // Entries without a string name are dropped, not cached.
        let odd = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":""},{"name":7},{"description":"x"},{"name":"ok","outputSchema":{}}]}}"#;
        assert_eq!(
            read_tools_list(200, odd).unwrap(),
            vec![json!({ "name": "ok" })]
        );
        let _ = GMAIL;
    }
}
