// The Google Workspace MCP pack as registry rows: one `mcp_servers` row per
// product an admin enables, named `google-<product>`, `auth_mode = 'google'`.
// The row's agents reach Google through the MCP gateway like any other
// server; the bearer is the calling agent's principal's own Google grant,
// vended through the one door (talaria-google-agent) and injected server-side.
//
// Where a row's calls go is this module's answer (`endpoint`), never the
// row's stored URL: the bearer only ever travels to Google's own endpoint for
// the product. The one exception is a DEBUG build's test override, below.

use crate::{PRODUCTS, Product};

/// The registry `auth_mode` of a pack row.
pub const AUTH_MODE: &str = "google";

/// The prefix of a pack row's registry name.
pub const SERVER_PREFIX: &str = "google-";

/// The product with Talaria's short name `id`.
pub fn product(id: &str) -> Option<Product> {
    PRODUCTS.iter().copied().find(|p| p.id == id)
}

/// A product's registry name: `google-<id>`.
pub fn server_name(p: &Product) -> String {
    format!("{SERVER_PREFIX}{}", p.id)
}

/// The product a pack row's registry name names.
pub fn product_of_server(name: &str) -> Option<Product> {
    name.strip_prefix(SERVER_PREFIX).and_then(product)
}

/// The row's label in the registry and on agents' cards.
pub fn server_label(p: &Product) -> String {
    format!("{} (Google MCP, Developer Preview)", p.name)
}

/// Google's Developer Preview terms, the clause an admin acknowledges before
/// enabling a server (https://developers.google.com/workspace/preview, read
/// 2026-11-12). Quoted, never reworded.
pub const PREVIEW_TERMS: &str = "features in Developer Preview may not be included in public applications prior to the General Availability announcement.";

/// Where the terms and the enrolment form are.
pub const PREVIEW_TERMS_URL: &str = "https://developers.google.com/workspace/preview";

/// The test override's variable (DEBUG builds only).
pub const TEST_ORIGINS_ENV: &str = "TALARIA_TEST_GOOGLE_MCP_ORIGINS";

/// The origin a product's calls go to: `https://<mcp service>`, or the
/// DEBUG-build override.
pub fn origin(p: &Product) -> String {
    #[cfg(debug_assertions)]
    if let Some(o) = test_origin(p.id) {
        return o;
    }
    format!("https://{}", host_of(p))
}

/// The Streamable HTTP endpoint the gateway relays a product's calls to.
pub fn endpoint(p: &Product) -> String {
    format!("{}/mcp/v1", origin(p))
}

/// The product's per-tool protected-resource document (RFC 9728), which
/// lists the scopes Google accepts for that tool — the URL Google's own 401
/// names in `WWW-Authenticate: Bearer resource_metadata="…"`.
pub fn resource_metadata_url(p: &Product, tool: &str) -> String {
    format!("{}/.well-known/oauth-protected-resource/{tool}", origin(p))
}

fn host_of(p: &Product) -> &'static str {
    p.endpoint
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or(p.endpoint)
}

/// TEST ONLY, and compiled into DEBUG builds only (`talaria dev`'s `cargo
/// run`, CI's `--api-from-ci` artifact): `TALARIA_TEST_GOOGLE_MCP_ORIGINS=
/// gmail=http://127.0.0.1:7811,calendar=http://127.0.0.1:7812` sends those
/// products' calls to a stub standing in for Google on a worktree stack.
/// Only a loopback `http://` origin is taken — a bearer never leaves the
/// machine through it — and anything else in the variable is ignored. A
/// release build has no such variable.
#[cfg(debug_assertions)]
fn test_origin(product_id: &str) -> Option<String> {
    let spec = std::env::var(TEST_ORIGINS_ENV).ok()?;
    parse_test_origins(&spec)
        .into_iter()
        .find(|(id, _)| id == product_id)
        .map(|(_, o)| o)
}

/// The override's `<product>=<origin>` pairs that name a loopback origin.
#[cfg(any(debug_assertions, test))]
fn parse_test_origins(spec: &str) -> Vec<(String, String)> {
    spec.split([',', ' '])
        .filter_map(|pair| {
            let (id, origin) = pair.trim().split_once('=')?;
            let origin = origin.trim().trim_end_matches('/');
            let u = url::Url::parse(origin).ok()?;
            let loopback = matches!(
                u.host(),
                Some(url::Host::Ipv4(ip)) if ip.is_loopback()
            ) || matches!(u.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback())
                || u.host_str() == Some("localhost");
            (u.scheme() == "http" && loopback && u.path() == "/" && u.query().is_none())
                .then(|| (id.trim().to_string(), origin.to_string()))
        })
        .collect()
}

/// Is `raw` one of Google's Workspace MCP servers? Any host whose name before
/// `.googleapis.com` ends in `mcp` (gmailmcp, drivemcp, calendarmcp, docsmcp,
/// sheetsmcp, slidesmcp, chatmcp, classroommcp, workspacemcp — and any Google
/// adds in that shape), and People's `/mcp` path. The custom-URL form refuses
/// them: registered by URL they would take no Google grant and answer every
/// agent with Google's raw 401, so they come in through the pack instead.
/// Google Cloud's IAM-governed servers (`bigquery.googleapis.com/mcp`, …)
/// are not these, and pass.
pub fn is_google_workspace_mcp_url(raw: &str) -> bool {
    let Ok(u) = url::Url::parse(raw.trim()) else {
        return false;
    };
    let Some(host) = u.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let Some(sub) = host.strip_suffix(".googleapis.com") else {
        return false;
    };
    let last = sub.rsplit('.').next().unwrap_or(sub);
    if last.ends_with("mcp") {
        return true;
    }
    last == "people"
        && u.path()
            .to_ascii_lowercase()
            .trim_start_matches('/')
            .split('/')
            .next()
            == Some("mcp")
}

/// The custom-URL form's refusal for a Google Workspace MCP URL.
pub const CUSTOM_URL_REFUSAL: &str = "Google's Workspace MCP servers are added through the Google Workspace MCP pack (Admin → Org → Google Workspace MCP, Developer Preview), which acts with each agent's own Google connection. Registered here they would take no Google grant, and every call would come back as Google's refusal.";

/// The gateway's sentence for a tool it will not relay. `held` is a tool
/// whose default is HOLD (refused while holds are not built); otherwise the
/// tool is denied outright. Says nothing was sent.
pub fn refusal_sentence(p: &Product, tool: &str, held: bool) -> String {
    if held {
        format!(
            "\u{201c}{tool}\u{201d} on Google's {} MCP server is not one of the read-only tools this workspace runs through Google's MCP servers. A tool that changes something needs a person to approve each call, and that approval is not available for Google's MCP servers yet. Nothing was sent to Google. To change something in Google Workspace, use the talaria toolkit's own Google tools (draft_email, draft_calendar_event, update_google_doc and the rest), which follow this workspace's approval rules.",
            p.name
        )
    } else {
        format!(
            "\u{201c}{tool}\u{201d} on Google's {} MCP server is never run for an agent here: Talaria does not let an agent move mail to or out of the trash, mark or unmark spam, or apply a sensitive label through Google's MCP servers. Nothing was sent to Google.",
            p.name
        )
    }
}

/// The gateway's sentence for a tool call carrying a workspace secret
/// handle: never spent into Google's servers.
pub const SECRET_HANDLE_REFUSAL: &str = "This call names a workspace secret («secret:…»). Secrets are never sent to Google's Workspace MCP servers. Nothing was sent to Google.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GMAIL;

    #[test]
    fn names_round_trip_and_unknown_products_are_none() {
        for p in PRODUCTS {
            assert_eq!(product(p.id), Some(p));
            assert_eq!(product_of_server(&server_name(&p)), Some(p));
        }
        assert_eq!(server_name(&GMAIL), "google-gmail");
        assert_eq!(product_of_server("gmail"), None);
        assert_eq!(product_of_server("google-classroom"), None);
        assert_eq!(product_of_server("google-"), None);
    }

    #[test]
    fn calls_go_to_googles_own_endpoint() {
        // The override is unset in the test process.
        for p in PRODUCTS {
            assert_eq!(endpoint(&p), p.endpoint);
            assert!(resource_metadata_url(&p, "x").starts_with("https://"));
        }
        assert_eq!(
            resource_metadata_url(&GMAIL, "list_labels"),
            "https://gmailmcp.googleapis.com/.well-known/oauth-protected-resource/list_labels"
        );
    }

    #[test]
    fn the_test_override_takes_only_loopback_http_origins() {
        assert_eq!(
            parse_test_origins(
                "gmail=http://127.0.0.1:7811, calendar=http://localhost:7812/ drive=http://[::1]:7813"
            ),
            vec![
                ("gmail".to_string(), "http://127.0.0.1:7811".to_string()),
                ("calendar".to_string(), "http://localhost:7812".to_string()),
                ("drive".to_string(), "http://[::1]:7813".to_string()),
            ]
        );
        for refused in [
            "gmail=https://127.0.0.1:7811",
            "gmail=http://10.0.0.5:7811",
            "gmail=http://evil.example:7811",
            "gmail=http://127.0.0.1.nip.io:7811",
            "gmail=http://127.0.0.1:7811/path",
            "gmail=http://127.0.0.1:7811/?q=1",
            "gmail",
            "",
        ] {
            assert!(parse_test_origins(refused).is_empty(), "{refused}");
        }
    }

    #[test]
    fn the_custom_url_guard_names_googles_workspace_servers_only() {
        for refused in [
            "https://gmailmcp.googleapis.com/mcp/v1",
            "https://GMAILMCP.googleapis.com./mcp/v1",
            "https://drivemcp.googleapis.com/mcp/v1",
            "https://calendarmcp.googleapis.com/mcp/v1",
            "https://docsmcp.googleapis.com/mcp/v1",
            "https://sheetsmcp.googleapis.com/mcp",
            "https://slidesmcp.googleapis.com/mcp/v1",
            "https://chatmcp.googleapis.com/mcp/v1",
            "https://classroommcp.googleapis.com/mcp/v1",
            "https://workspacemcp.googleapis.com/mcp/v1",
            "https://people.googleapis.com/mcp/v1",
            "https://people.googleapis.com/MCP/v1",
            "http://gmailmcp.googleapis.com:443/anything",
        ] {
            assert!(is_google_workspace_mcp_url(refused), "{refused}");
        }
        for passed in [
            "https://people.googleapis.com/v1/people/me",
            "https://bigquery.googleapis.com/mcp",
            "https://run.googleapis.com/mcp",
            "https://gmailmcp.googleapis.com.evil.example/mcp/v1",
            "https://evil.example/gmailmcp.googleapis.com/mcp/v1",
            "https://mcp.notion.com/mcp",
            "not a url",
            "talaria-pkg://gmailmcp",
        ] {
            assert!(!is_google_workspace_mcp_url(passed), "{passed}");
        }
    }

    #[test]
    fn every_refusal_says_nothing_was_sent() {
        for held in [true, false] {
            assert!(refusal_sentence(&GMAIL, "trash_thread", held).contains("Nothing was sent"));
        }
        assert!(SECRET_HANDLE_REFUSAL.contains("Nothing was sent"));
    }
}
