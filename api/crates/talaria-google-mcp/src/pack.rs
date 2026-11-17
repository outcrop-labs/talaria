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

/// TEST ONLY: `TALARIA_TEST_GOOGLE_MCP_ORIGINS=gmail=http://127.0.0.1:7811,
/// calendar=http://127.0.0.1:7812` sends those products' calls to a stub
/// standing in for Google on a worktree stack. Compiled into debug-profile
/// builds only — `talaria dev`'s `cargo run`, CI's `--api-from-ci` artifact,
/// and also the nightly and rc channel images, which `release.yml` builds
/// with the dev profile (api/package.Dockerfile) — and READ only outside a
/// deployment: a process carrying the published image's own statement
/// (`TALARIA_INSTALL=image`) or a production checkout's
/// (`TALARIA_RUNTIME=prod-server`, set by ui/server-entry.ts) ignores it, so
/// no deployed instance honours it whatever its profile. Only a loopback
/// `http://` origin is taken — a bearer never leaves the machine through it
/// — and anything else in the variable is ignored. A release-profile build
/// (main's feed, every stable tag) has no such code at all.
#[cfg(debug_assertions)]
fn test_origin(product_id: &str) -> Option<String> {
    if deployed(
        std::env::var("TALARIA_INSTALL").ok().as_deref(),
        std::env::var("TALARIA_RUNTIME").ok().as_deref(),
    ) {
        return None;
    }
    let spec = std::env::var(TEST_ORIGINS_ENV).ok()?;
    parse_test_origins(&spec)
        .into_iter()
        .find(|(id, _)| id == product_id)
        .map(|(_, o)| o)
}

/// Is this process a deployment — the published image, or a checkout run
/// as the production server? The same two signals `talaria-users`'
/// `compile_strict` and the updater's install mode read.
#[cfg(any(debug_assertions, test))]
fn deployed(install: Option<&str>, runtime: Option<&str>) -> bool {
    install == Some("image") || runtime == Some("prod-server")
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

/// The domains Google answers its Workspace MCP servers under. Each server
/// has several hostnames: probed with no credential on 2026-11-17
/// (`fixtures/alias-hosts.json`), a `tools/list` POST to `/mcp/v1` was
/// answered by 49 hostnames, each with its server's committed catalog byte
/// for byte: `<server>.googleapis.com`, `<server>.clients6.google.com` and
/// `<server>.mtls.clients6.google.com` for every `*mcp` server,
/// `<server>.mtls.googleapis.com` for all but Chat, and
/// `content-<server>[.mtls].googleapis.com` for Gmail, Calendar, Chat,
/// Universal Search and Classroom; People's `/mcp` at `people`, `people.mtls`,
/// `content-people` and `content-people.mtls` under `googleapis.com`.
const GOOGLE_DOMAINS: [&str; 2] = ["googleapis.com", "google.com"];

/// Is `raw` one of Google's Workspace MCP servers, under any of its names?
/// A host under `googleapis.com` or `google.com` with any label ending in
/// `mcp` (gmailmcp, drivemcp, calendarmcp, docsmcp, sheetsmcp, slidesmcp,
/// chatmcp, classroommcp, workspacemcp — and any Google adds in that shape —
/// under the plain, `mtls`, `clients6` and `content-` names; see
/// `GOOGLE_DOMAINS`), and a host there with a label ending in `people` on an
/// `/mcp` path. Matching every label, not one position, is deliberate: the
/// service's label sits first in every shape Google was seen to serve, and
/// over-refusing is the safe direction. The custom-URL form refuses them, and
/// the gateway will not relay to a row that names one (`UNPACKED_ROW_REFUSAL`):
/// by URL they would take no Google grant, or an attached one with none of
/// the pack's policy. Google Cloud's IAM-governed servers
/// (`bigquery.googleapis.com/mcp`, …) are not these, and pass.
pub fn is_google_workspace_mcp_url(raw: &str) -> bool {
    let Ok(u) = url::Url::parse(raw.trim()) else {
        return false;
    };
    let Some(host) = u.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let Some(sub) = GOOGLE_DOMAINS
        .iter()
        .find_map(|d| host.strip_suffix(d).and_then(|s| s.strip_suffix('.')))
    else {
        return false;
    };
    if sub.split('.').any(|l| l.ends_with("mcp")) {
        return true;
    }
    sub.split('.').any(|l| l.ends_with("people"))
        && u.path()
            .to_ascii_lowercase()
            .trim_start_matches('/')
            .split('/')
            .next()
            == Some("mcp")
}

/// The gateway's answer for a registry row that is not the pack's but names
/// one of Google's Workspace MCP servers — one registered before the
/// custom-URL guard, or under a name the guard did not know then. Nothing is
/// relayed to it, with or without a credential.
pub const UNPACKED_ROW_REFUSAL: &str = "This server's address is one of Google's Workspace MCP servers, which run here only through the Google Workspace MCP pack (Admin → Org → Google Workspace MCP, Developer Preview), where each call acts with the agent's own Google connection and only read-only tools run. Nothing was sent to Google's Workspace MCP servers.";

/// The custom-URL form's refusal for a Google Workspace MCP URL.
pub const CUSTOM_URL_REFUSAL: &str = "Google's Workspace MCP servers are added through the Google Workspace MCP pack (Admin → Org → Google Workspace MCP, Developer Preview), which acts with each agent's own Google connection. Registered here they would take no Google grant, and every call would come back as Google's refusal.";

/// The gateway's sentence for a tool it will not relay. `held` is a tool
/// whose default is HOLD (refused while holds are not built); otherwise the
/// tool is denied outright. Says nothing was sent to Google's Workspace MCP
/// servers — not "to Google": the token was vended first, and its refresh
/// may already have reached Google's token endpoint.
pub fn refusal_sentence(p: &Product, tool: &str, held: bool) -> String {
    if held {
        format!(
            "\u{201c}{tool}\u{201d} on Google's {} MCP server is not one of the read-only tools this workspace runs through Google's MCP servers. A tool that changes something needs a person to approve each call, and that approval is not available for Google's MCP servers yet. Nothing was sent to Google's Workspace MCP servers. To change something in Google Workspace, use the talaria toolkit's own Google tools (draft_email, draft_calendar_event, update_google_doc and the rest), which follow this workspace's approval rules.",
            p.name
        )
    } else {
        format!(
            "\u{201c}{tool}\u{201d} on Google's {} MCP server is never run for an agent here: Talaria does not let an agent move mail to or out of the trash, mark or unmark spam, or apply a sensitive label through Google's MCP servers. Nothing was sent to Google's Workspace MCP servers.",
            p.name
        )
    }
}

/// The gateway's sentence for a tool call carrying a workspace secret
/// handle: never spent into Google's servers.
pub const SECRET_HANDLE_REFUSAL: &str = "This call names a workspace secret («secret:…»). Secrets are never spent into Google's Workspace MCP servers, so nothing was sent to them.";

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
    fn a_deployment_never_reads_the_test_override() {
        // The published image (nightly and rc are dev-profile builds, so
        // they carry the code) and a production checkout ignore it.
        assert!(deployed(Some("image"), None));
        assert!(deployed(None, Some("prod-server")));
        assert!(deployed(Some("image"), Some("prod-server")));
        // `talaria dev` (vite) and CI's artifact run set neither.
        assert!(!deployed(None, None));
        assert!(!deployed(Some("checkout"), Some("dev")));
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
            // The same servers under Google's other names. Each of these
            // answered a credential-less `tools/list` with the server's
            // catalog when probed on 2026-11-17 (the Gmail ones byte-identical
            // to fixtures/tools-list/gmailmcp.json, People's to people.json).
            "https://gmailmcp.mtls.googleapis.com/mcp/v1",
            "https://gmailmcp.clients6.google.com/mcp/v1",
            "https://GMAILMCP.CLIENTS6.GOOGLE.COM/mcp/v1",
            "https://gmailmcp.clients6.google.com./mcp/v1",
            "https://gmailmcp.mtls.clients6.google.com/mcp/v1",
            "https://content-gmailmcp.googleapis.com/mcp/v1",
            "https://content-gmailmcp.mtls.googleapis.com/mcp/v1",
            "https://calendarmcp.clients6.google.com/mcp/v1",
            "https://drivemcp.clients6.google.com/mcp/v1",
            "https://chatmcp.clients6.google.com/mcp/v1",
            "https://workspacemcp.clients6.google.com/mcp/v1",
            "https://classroommcp.clients6.google.com/mcp/v1",
            "https://sheetsmcp.mtls.googleapis.com/mcp/v1",
            "https://people.mtls.googleapis.com/mcp/v1",
            "https://content-people.googleapis.com/mcp/v1",
            "https://content-people.mtls.googleapis.com/mcp/v1",
            // Shapes not seen to answer (no DNS, or 404), refused all the
            // same: over-refusing is the safe direction.
            "https://x.gmailmcp.googleapis.com/mcp/v1",
            "https://gmailmcp.us-east1.rep.googleapis.com/mcp/v1",
            "https://people.clients6.google.com/mcp/v1",
        ] {
            assert!(is_google_workspace_mcp_url(refused), "{refused}");
        }
        for passed in [
            "https://people.googleapis.com/v1/people/me",
            "https://people.clients6.google.com/v1/people/me",
            "https://bigquery.googleapis.com/mcp",
            "https://bigquery.mtls.googleapis.com/mcp",
            "https://bigquery.clients6.google.com/mcp",
            "https://run.googleapis.com/mcp",
            "https://gmailmcp.googleapis.com.evil.example/mcp/v1",
            "https://gmailmcp.clients6.google.com.evil.example/mcp/v1",
            "https://gmailmcp.evilgoogle.com/mcp/v1",
            "https://gmailmcp.evilgoogleapis.com/mcp/v1",
            "https://evil.example/gmailmcp.googleapis.com/mcp/v1",
            "https://gmailmcp.googleapis.com@evil.example/mcp/v1",
            "https://googleapis.com/mcp/v1",
            "https://mcp.notion.com/mcp",
            "not a url",
            "talaria-pkg://gmailmcp",
        ] {
            assert!(!is_google_workspace_mcp_url(passed), "{passed}");
        }
    }

    /// Every hostname Google answered a credential-less `tools/list` on, in
    /// the alias probe of 2026-11-17 (`fixtures/alias-hosts.json`: 49 of the
    /// 65 names probed, across the plain, `mtls`, `clients6`, `mtls.clients6`
    /// and `content-` shapes), is refused by the custom-URL guard. A body the
    /// probe matched to a committed catalog names one that exists.
    #[test]
    fn every_name_google_was_seen_to_serve_a_workspace_server_by_is_refused() {
        let probe: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/alias-hosts.json")).unwrap();
        let answers = probe["answers"].as_array().unwrap();
        let served: Vec<&str> = answers
            .iter()
            .filter(|a| a["status"] == 200)
            .map(|a| a["url"].as_str().unwrap())
            .collect();
        assert_eq!(served.len(), 49, "the probe's answering hosts");
        for url in &served {
            assert!(is_google_workspace_mcp_url(url), "{url}");
        }
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        for same in answers.iter().filter_map(|a| a["sameAs"].as_str()) {
            assert!(fixtures.join(same).is_file(), "{same}");
        }
        // Every product's own host is among them.
        for p in PRODUCTS {
            assert!(served.contains(&p.endpoint), "{}", p.endpoint);
        }
    }

    #[test]
    fn every_refusal_says_nothing_was_sent() {
        for held in [true, false] {
            assert!(
                refusal_sentence(&GMAIL, "trash_thread", held)
                    .contains("Nothing was sent to Google's Workspace MCP servers.")
            );
        }
        assert!(SECRET_HANDLE_REFUSAL.contains("nothing was sent to them"));
    }
}
