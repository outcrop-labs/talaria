// Where an OAuth app gets created, per provider — the setup details a person
// needs when a server will not register Talaria's callback by itself (no
// dynamic client registration, or a registration that refuses hosted
// callbacks). Every provider's own `service_documentation` link still wins
// when discovery found one; this map adds the developer-portal door and the
// one or two facts about that portal that cost people an afternoon.
//
// FACTUAL, NOT EDITORIAL: a row is a documented developer-portal URL plus
// what that portal calls the thing to create. A provider absent here still
// gets the generic guidance (register the callback, paste the client id and
// secret) — the map is a convenience, never a ceiling on which servers can
// be connected. Keyed by registrable domain; the cross-domain issuers the
// pinning layer already knows (api.githubcopilot.com → github.com) resolve
// to the issuer's row.

use serde_json::{Value, json};

pub struct Guide {
    /// The developer portal where the app is created.
    pub portal: &'static str,
    /// What the portal calls it ("OAuth App", "Connected App", "Integration").
    pub app_name: &'static str,
    /// The facts that bite: where the callback field is, what the client
    /// secret is called, whether scopes are picked on the app or at authorize.
    pub notes: &'static [&'static str],
}

const GUIDES: &[(&str, Guide)] = &[
    (
        "github.com",
        Guide {
            portal: "https://github.com/settings/developers",
            app_name: "OAuth App",
            notes: &[
                "Settings → Developer settings → OAuth Apps → New OAuth App.",
                "Authorization callback URL is the callback below, exactly.",
                "Generate a client secret after creating the app; GitHub shows it once.",
                "For an organization's repos, create the app under the org (Organization settings → Developer settings) or enable third-party access for it.",
            ],
        },
    ),
    (
        "figma.com",
        Guide {
            portal: "https://www.figma.com/developers/apps",
            app_name: "App",
            notes: &[
                "Create app, then add the callback under the app's OAuth 2.0 settings.",
                "The client secret is shown once at creation.",
            ],
        },
    ),
    (
        "asana.com",
        Guide {
            portal: "https://app.asana.com/0/my-apps",
            app_name: "App",
            notes: &[
                "Developer console → My apps → Create new app → OAuth → add the callback as a redirect URL.",
            ],
        },
    ),
    (
        "atlassian.com",
        Guide {
            portal: "https://developer.atlassian.com/console/myapps/",
            app_name: "OAuth 2.0 integration",
            notes: &[
                "Create → OAuth 2.0 integration; Authorization → add the callback URL.",
                "Permissions must be added to the app for each product (Jira, Confluence) before the scopes authorize.",
                "Distribution must be set to Sharing for anyone outside your own site to connect.",
            ],
        },
    ),
    (
        "slack.com",
        Guide {
            portal: "https://api.slack.com/apps",
            app_name: "Slack app",
            notes: &[
                "Create New App → From scratch; OAuth & Permissions → Redirect URLs → add the callback.",
                "Slack requires https callbacks — a verified hosting domain is needed (Admin → Org).",
                "Bot token scopes are picked on the app, not at authorize time.",
            ],
        },
    ),
    (
        "google.com",
        Guide {
            portal: "https://console.cloud.google.com/apis/credentials",
            app_name: "OAuth client ID (Web application)",
            notes: &[
                "Configure the OAuth consent screen first; add the callback as an Authorized redirect URI.",
                "Enable the APIs the server's scopes need under APIs & Services.",
            ],
        },
    ),
    (
        "notion.so",
        Guide {
            portal: "https://www.notion.so/my-integrations",
            app_name: "Public integration",
            notes: &[
                "New integration → type Public; the callback goes under OAuth Domain & URIs → Redirect URIs.",
                "The OAuth client ID and secret are on the integration's Configuration tab.",
            ],
        },
    ),
    (
        // Notion's MCP lives on mcp.notion.com; the same portal.
        "notion.com",
        Guide {
            portal: "https://www.notion.so/my-integrations",
            app_name: "Public integration",
            notes: &[
                "New integration → type Public; the callback goes under OAuth Domain & URIs → Redirect URIs.",
                "The OAuth client ID and secret are on the integration's Configuration tab.",
            ],
        },
    ),
    (
        "linear.app",
        Guide {
            portal: "https://linear.app/settings/api/applications",
            app_name: "OAuth application",
            notes: &["Settings → API → OAuth applications → Create new; add the callback URL."],
        },
    ),
    (
        "hubspot.com",
        Guide {
            portal: "https://developers.hubspot.com/",
            app_name: "App (developer account)",
            notes: &[
                "Apps → Create app → Auth tab: add the callback as a redirect URL and pick the scopes the server needs.",
            ],
        },
    ),
    (
        "salesforce.com",
        Guide {
            portal: "https://login.salesforce.com/",
            app_name: "External Client App (or Connected App)",
            notes: &[
                "Setup → App Manager → New; enable OAuth settings, add the callback, select the scopes.",
                "Consumer key = client id, consumer secret = client secret.",
            ],
        },
    ),
    (
        "gitlab.com",
        Guide {
            portal: "https://gitlab.com/-/user_settings/applications",
            app_name: "Application",
            notes: &[
                "Add the callback as the Redirect URI; untick Confidential only if the server's metadata says so.",
            ],
        },
    ),
    (
        "dropbox.com",
        Guide {
            portal: "https://www.dropbox.com/developers/apps",
            app_name: "App",
            notes: &["Create app → Settings → OAuth 2 → Redirect URIs → add the callback."],
        },
    ),
    (
        "box.com",
        Guide {
            portal: "https://app.box.com/developers/console",
            app_name: "Custom App (User Authentication / OAuth 2.0)",
            notes: &[
                "Configuration → OAuth 2.0 Redirect URI → add the callback; the app may need admin authorization in the Box admin console.",
            ],
        },
    ),
    (
        "zoom.us",
        Guide {
            portal: "https://marketplace.zoom.us/develop/create",
            app_name: "General App (OAuth)",
            notes: &["Add the callback under OAuth Redirect URL and in the OAuth allow list."],
        },
    ),
    (
        "microsoft.com",
        Guide {
            portal: "https://portal.azure.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade",
            app_name: "App registration",
            notes: &[
                "Azure portal → App registrations → New registration; platform Web, redirect URI = the callback.",
                "Certificates & secrets → New client secret; API permissions → add the Graph scopes the server needs.",
            ],
        },
    ),
    (
        "intercom.com",
        Guide {
            portal: "https://app.intercom.com/a/apps/_/developer-hub",
            app_name: "App",
            notes: &[
                "Developer Hub → New app → Authentication → add the callback as a redirect URL.",
            ],
        },
    ),
    (
        "monday.com",
        Guide {
            portal: "https://monday.com/developers/apps",
            app_name: "App",
            notes: &[
                "Create app → OAuth → add the callback to Redirect URLs; scopes are set on the app.",
            ],
        },
    ),
    (
        "airtable.com",
        Guide {
            portal: "https://airtable.com/create/oauth",
            app_name: "OAuth integration",
            notes: &[
                "Register the callback as an OAuth redirect URL; scopes are chosen on the integration.",
            ],
        },
    ),
    (
        "canva.com",
        Guide {
            portal: "https://www.canva.com/developers/",
            app_name: "Integration",
            notes: &[
                "Developer portal → Your integrations → Create; Authentication → add the callback as a redirect URL.",
            ],
        },
    ),
    (
        "shopify.com",
        Guide {
            portal: "https://partners.shopify.com/",
            app_name: "App (Partner Dashboard)",
            notes: &[
                "Apps → Create app; App setup → URLs → Allowed redirection URL(s) → add the callback.",
            ],
        },
    ),
    (
        "sentry.io",
        Guide {
            portal: "https://sentry.io/settings/account/api/applications/",
            app_name: "API Application",
            notes: &["Add the callback under Authorized Redirect URIs."],
        },
    ),
    (
        "supabase.com",
        Guide {
            portal: "https://supabase.com/dashboard/org/_/apps",
            app_name: "OAuth App",
            notes: &[
                "Organization settings → OAuth Apps → Add application; add the callback as an authorization callback URL.",
            ],
        },
    ),
    (
        "vercel.com",
        Guide {
            portal: "https://vercel.com/dashboard/integrations/console",
            app_name: "Integration",
            notes: &[
                "Integrations console → Create; the callback goes in Redirect URL.",
                "Vercel's automatic registration only accepts localhost callbacks, which is why a hosted Talaria lands here.",
            ],
        },
    ),
];

/// Same approximation the pinning layer uses: two labels, three under the
/// common multi-label suffixes. Duplicated rather than shared because the
/// pin's version is private to its module and this one is not security
/// relevant — a miss here means generic guidance, nothing worse.
fn registrable(host: &str) -> String {
    let h = host.to_lowercase();
    let labels: Vec<&str> = h.trim_end_matches('.').split('.').collect();
    if labels.len() <= 2 {
        return labels.join(".");
    }
    let last2 = labels[labels.len() - 2..].join(".");
    const MULTI: [&str; 6] = ["co.uk", "com.au", "co.jp", "co.nz", "com.br", "co.in"];
    if MULTI.contains(&last2.as_str()) {
        labels[labels.len() - 3..].join(".")
    } else {
        last2
    }
}

/// The guide for a server, by the registrable domain of its URL — or of its
/// authorization endpoint when discovery found one (GitHub's MCP lives on
/// githubcopilot.com; its OAuth app is created on github.com).
pub fn guide_for(server_url: &str, authorization_endpoint: Option<&str>) -> Option<Value> {
    let mut candidates: Vec<String> = Vec::new();
    for u in [authorization_endpoint, Some(server_url)]
        .into_iter()
        .flatten()
    {
        if let Some(h) = reqwest::Url::parse(u)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
        {
            candidates.push(registrable(&h));
        }
    }
    // Documented cross-domain issuers: the MCP host's registrable domain maps
    // to the portal's.
    if candidates.iter().any(|c| c == "githubcopilot.com") {
        candidates.push("github.com".into());
    }
    let (domain, g) = candidates
        .iter()
        .find_map(|c| GUIDES.iter().find(|(d, _)| d == c))?;
    Some(json!({
        "domain": domain,
        "portal": g.portal,
        "appName": g.app_name,
        "notes": g.notes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guides_resolve_by_server_domain_and_by_issuer() {
        let g = guide_for("https://mcp.linear.app/mcp", None).expect("linear is listed");
        assert_eq!(g["domain"], "linear.app");
        assert!(
            g["portal"]
                .as_str()
                .unwrap()
                .starts_with("https://linear.app/")
        );
        // GitHub's MCP host is not github.com; the issuer is.
        let g = guide_for(
            "https://api.githubcopilot.com/mcp/",
            Some("https://github.com/login/oauth/authorize"),
        )
        .expect("github by issuer");
        assert_eq!(g["domain"], "github.com");
        // …and even without a discovered issuer, the documented cross-domain
        // pin resolves it.
        let g = guide_for("https://api.githubcopilot.com/mcp/", None).expect("github by pin");
        assert_eq!(g["domain"], "github.com");
        // An unknown provider is generic guidance, not an error.
        assert!(guide_for("https://mcp.example.com/mcp", None).is_none());
        assert!(guide_for("not a url", None).is_none());
    }

    #[test]
    fn every_guide_has_a_portal_an_app_name_and_at_least_one_note() {
        for (domain, g) in GUIDES {
            assert!(g.portal.starts_with("https://"), "{domain}");
            assert!(!g.app_name.is_empty(), "{domain}");
            assert!(!g.notes.is_empty(), "{domain}");
            assert_eq!(registrable(domain), *domain, "{domain} is registrable");
        }
    }
}
