// The featured shelf's EDITORIAL list, as the operator keeps it.
//
// The shelf used to be `FEATURED_DOMAINS` and nothing else: a company an org
// actually runs on but that is not on the list (or that publishes its server
// somewhere the registry does not know) could not be featured without a
// Rust release, and a built-in the org will never use could not be dropped.
// The operator's edits live in app_settings under `mcp_featured`:
//
//   { "domains":  ["deepwiki.com"],            added to the shelf, after the built-ins
//     "hidden":   ["paypal.com"],              built-ins (or added ones) taken off it
//     "standIns": [{ "domain", "title", "url", "description"? }] }
//
// A stand-in is the operator's own answer to "which endpoint is this
// publisher's server" — it puts its domain on the shelf by itself and wins
// over the registry's row and the built-in documented map alike, because the
// person who typed it knows something the feed does not. The MERGE is pure
// (`merged_domains`, `stand_in`), so the rules are pinned by tests rather
// than by the database.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;

use crate::{FEATURED_DOMAINS, LibraryServer, Tier};

pub const FEATURED_KEY: &str = "mcp_featured";
/// Bounds that keep the shelf a shelf: the built-in list is 32 long.
const MAX_DOMAINS: usize = 100;
const MAX_STAND_INS: usize = 50;
const MAX_TITLE: usize = 80;
const MAX_DESCRIPTION: usize = 220;
/// Operator stand-ins are stored under this registry-name prefix, so the
/// mirror row says where it came from and the prune can find its own.
pub const OPERATOR_PREFIX: &str = "op:";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StandIn {
    pub domain: String,
    pub title: String,
    pub url: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FeaturedConfig {
    pub domains: Vec<String>,
    pub hidden: Vec<String>,
    pub stand_ins: Vec<StandIn>,
}

/// A bare hostname: labels of [a-z0-9-], at least two of them, no scheme,
/// port, path or wildcard. What the shelf joins on (`mcp_library_entries.domain`).
pub fn is_bare_host(h: &str) -> bool {
    let labels: Vec<&str> = h.split('.').collect();
    h.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        // The last label is a TLD, never all digits — "10.0.0.1" is an
        // address, not a publisher.
        && !labels
            .last()
            .is_some_and(|l| l.chars().all(|c| c.is_ascii_digit()))
}

fn norm_host(raw: &str) -> String {
    raw.trim().trim_end_matches('.').to_lowercase()
}

fn push_unique(out: &mut Vec<String>, d: String) {
    if !out.contains(&d) {
        out.push(d);
    }
}

/// Normalize and check what an operator sent. Err is the sentence the PUT
/// answers with — it names the field and the value that failed.
pub fn validate(input: FeaturedConfig) -> Result<FeaturedConfig, String> {
    let host = |field: &str, raw: &str| -> Result<String, String> {
        let h = norm_host(raw);
        if is_bare_host(&h) {
            Ok(h)
        } else {
            Err(format!(
                "{field}: \"{}\" is not a bare host name — write it like linear.app, without https:// or a path",
                raw.trim()
            ))
        }
    };
    let mut out = FeaturedConfig::default();
    for d in &input.domains {
        push_unique(&mut out.domains, host("domains", d)?);
    }
    for d in &input.hidden {
        push_unique(&mut out.hidden, host("hidden", d)?);
    }
    for s in &input.stand_ins {
        let domain = host("standIns.domain", &s.domain)?;
        let title = s.title.trim().to_string();
        if title.is_empty() || title.chars().count() > MAX_TITLE {
            return Err(format!(
                "standIns: the stand-in for {domain} needs a title of 1-{MAX_TITLE} characters"
            ));
        }
        let url = s.url.trim().to_string();
        let parsed = reqwest::Url::parse(&url).ok();
        let ok = parsed
            .as_ref()
            .is_some_and(|u| u.scheme() == "https" && u.host_str().is_some())
            && !url.contains('{');
        if !ok {
            return Err(format!(
                "standIns: the stand-in for {domain} needs an https:// endpoint URL (got \"{url}\")"
            ));
        }
        // The URL is served to every member who opens the marketplace and
        // kept verbatim in the audit row, so it must carry no secret. Keys
        // in userinfo or the query string are exactly that; a server that
        // wants one gets it through the install's Credentials form, sealed.
        // The refusal does not echo the URL — it may be the secret.
        if let Some(u) = parsed.as_ref() {
            if !u.username().is_empty() || u.password().is_some() {
                return Err(format!(
                    "standIns: the stand-in for {domain} has a user name or password in its URL — remove it; credentials go in the server's Credentials form after install"
                ));
            }
            if u.query().is_some() || u.fragment().is_some() {
                return Err(format!(
                    "standIns: the stand-in for {domain} has a query string or fragment in its URL — remove it (every member sees this URL, so it must not carry a key); credentials go in the server's Credentials form after install"
                ));
            }
        }
        let description = s
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(|d| d.chars().take(MAX_DESCRIPTION).collect::<String>());
        if out.stand_ins.iter().any(|o| o.domain == domain) {
            return Err(format!(
                "standIns: {domain} has two stand-ins — keep one endpoint per publisher"
            ));
        }
        out.stand_ins.push(StandIn {
            domain,
            title,
            url,
            description,
        });
    }
    if out.domains.len() > MAX_DOMAINS {
        return Err(format!(
            "domains: at most {MAX_DOMAINS} added domains — the shelf is a shelf, search reaches the rest"
        ));
    }
    if out.stand_ins.len() > MAX_STAND_INS {
        return Err(format!("standIns: at most {MAX_STAND_INS} stand-ins"));
    }
    Ok(out)
}

/// The shelf's domains, in order: the built-ins (editorial order), then the
/// operator's added domains, then any stand-in domain not already named —
/// a stand-in IS a request to feature its publisher. `hidden` removes from
/// all three; hiding wins, so a domain both added and hidden stays off.
pub fn merged_domains(cfg: &FeaturedConfig) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let candidates = FEATURED_DOMAINS
        .iter()
        .map(|d| d.to_string())
        .chain(cfg.domains.iter().cloned())
        .chain(cfg.stand_ins.iter().map(|s| s.domain.clone()));
    for d in candidates {
        if !cfg.hidden.contains(&d) {
            push_unique(&mut out, d);
        }
    }
    out
}

/// Built-in or operator-added, for the editor's chips.
pub fn is_builtin(domain: &str) -> bool {
    FEATURED_DOMAINS.contains(&domain)
}

/// The operator's stand-in for a domain as a shelf entry — first-party (the
/// operator vouched for it), hosted, no declared headers (discovery at
/// install says whether it wants a key).
pub fn stand_in(cfg: &FeaturedConfig, domain: &str) -> Option<LibraryServer> {
    cfg.stand_ins
        .iter()
        .find(|s| s.domain == domain)
        .map(stand_in_server)
}

pub fn stand_in_server(s: &StandIn) -> LibraryServer {
    LibraryServer {
        registry_name: format!("{OPERATOR_PREFIX}{}", s.domain),
        title: s.title.clone(),
        description: s.description.clone(),
        url: s.url.clone(),
        domain: Some(s.domain.clone()),
        icon: None,
        tier: Tier::FirstParty,
        required_headers: Vec::new(),
        package: None,
    }
}

/// The stored config; an absent or unreadable row is "no edits" — the
/// built-in shelf, unchanged.
pub async fn load(pg: &PgPool) -> FeaturedConfig {
    let v = talaria_settings::get_setting(pg, FEATURED_KEY, Value::Null).await;
    serde_json::from_value(v).unwrap_or_default()
}

pub async fn save(pg: &PgPool, cfg: &FeaturedConfig) -> Result<(), String> {
    let v = serde_json::to_value(cfg).map_err(|e| e.to_string())?;
    talaria_settings::set_setting(pg, FEATURED_KEY, &v)
        .await
        .map_err(|e| e.to_string())
}

/// `{domains, hidden, standIns, builtins, shelf}` — the editor's read: what
/// the operator stored, the built-in list (so chips can say which is which),
/// and the merged shelf order the marketplace will show.
pub fn wire(cfg: &FeaturedConfig) -> Value {
    serde_json::json!({
        "domains": cfg.domains,
        "hidden": cfg.hidden,
        "standIns": cfg.stand_ins,
        "builtins": FEATURED_DOMAINS,
        "shelf": merged_domains(cfg),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg(v: Value) -> FeaturedConfig {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn no_edits_is_exactly_the_builtin_shelf() {
        let merged = merged_domains(&FeaturedConfig::default());
        assert_eq!(
            merged,
            FEATURED_DOMAINS
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_added_domain_lands_after_the_builtins_once() {
        let c = cfg(json!({ "domains": ["deepwiki.com", "github.com", "deepwiki.com"] }));
        let merged = merged_domains(&c);
        assert_eq!(merged.len(), FEATURED_DOMAINS.len() + 1);
        assert_eq!(merged.last().map(String::as_str), Some("deepwiki.com"));
        // github.com is a built-in: re-adding it neither duplicates nor moves it.
        assert_eq!(merged.iter().filter(|d| *d == "github.com").count(), 1);
        assert_eq!(merged[0], "github.com");
    }

    #[test]
    fn hiding_drops_a_builtin_and_wins_over_an_add() {
        let c = cfg(json!({
            "domains": ["deepwiki.com"],
            "hidden": ["paypal.com", "deepwiki.com"],
        }));
        let merged = merged_domains(&c);
        assert!(!merged.contains(&"paypal.com".to_string()));
        assert!(!merged.contains(&"deepwiki.com".to_string()));
        assert_eq!(merged.len(), FEATURED_DOMAINS.len() - 1);
        assert!(is_builtin("paypal.com") && !is_builtin("deepwiki.com"));
    }

    #[test]
    fn a_stand_in_features_its_domain_and_overrides_the_documented_map() {
        let c = cfg(json!({
            "standIns": [
                { "domain": "deepwiki.com", "title": "DeepWiki", "url": "https://mcp.deepwiki.com/mcp" },
                { "domain": "github.com", "title": "GitHub (enterprise)", "url": "https://gh.example.com/mcp" },
            ],
        }));
        let merged = merged_domains(&c);
        assert_eq!(merged.last().map(String::as_str), Some("deepwiki.com"));
        // github.com stays in its built-in slot, served by the operator's endpoint.
        assert_eq!(merged[0], "github.com");
        let gh = stand_in(&c, "github.com").expect("the stand-in answers for its domain");
        assert_eq!(gh.url, "https://gh.example.com/mcp");
        assert_eq!(gh.registry_name, "op:github.com");
        assert_eq!(gh.tier, Tier::FirstParty);
        assert!(gh.package.is_none());
        assert!(stand_in(&c, "linear.app").is_none());
        // A hidden domain's stand-in does not bring it back.
        let mut hidden = c.clone();
        hidden.hidden.push("deepwiki.com".into());
        assert!(!merged_domains(&hidden).contains(&"deepwiki.com".to_string()));
    }

    #[test]
    fn validation_normalizes_hosts_and_refuses_anything_else() {
        let ok = validate(cfg(json!({
            "domains": [" DeepWiki.com. ", "deepwiki.com"],
            "hidden": ["PayPal.com"],
            "standIns": [{ "domain": "Deepwiki.com", "title": " DeepWiki ",
                           "url": "https://mcp.deepwiki.com/mcp", "description": "  " }],
        })))
        .unwrap();
        assert_eq!(ok.domains, vec!["deepwiki.com"]);
        assert_eq!(ok.hidden, vec!["paypal.com"]);
        assert_eq!(ok.stand_ins[0].domain, "deepwiki.com");
        assert_eq!(ok.stand_ins[0].title, "DeepWiki");
        assert_eq!(ok.stand_ins[0].description, None);

        for bad in [
            "https://linear.app",
            "linear.app/mcp",
            "linear.app:443",
            "localhost",
            "*.linear.app",
            "-x.com",
            "10.0.0.1",
            "",
        ] {
            let e = validate(cfg(json!({ "domains": [bad] }))).unwrap_err();
            assert!(e.starts_with("domains:"), "{bad}: {e}");
        }
        assert!(
            validate(cfg(json!({ "hidden": ["not a host"] })))
                .unwrap_err()
                .starts_with("hidden:")
        );
        for url in [
            "http://mcp.x.com/mcp",
            "mcp.x.com/mcp",
            "https://{host}/mcp",
            "ftp://x.com",
        ] {
            let e = validate(cfg(json!({
                "standIns": [{ "domain": "x.com", "title": "X", "url": url }]
            })))
            .unwrap_err();
            assert!(e.contains("https://"), "{url}: {e}");
        }
        // Secrets never ride in a URL every member is served — and the
        // refusal does not repeat them.
        for (url, says) in [
            ("https://u:sk-secret@mcp.x.com/mcp", "user name or password"),
            ("https://sk-secret@mcp.x.com/mcp", "user name or password"),
            ("https://mcp.x.com/mcp?api_key=sk-secret", "query string"),
            ("https://mcp.x.com/mcp?", "query string"),
            (
                "https://mcp.x.com/mcp#sk-secret",
                "query string or fragment",
            ),
        ] {
            let e = validate(cfg(json!({
                "standIns": [{ "domain": "x.com", "title": "X", "url": url }]
            })))
            .unwrap_err();
            assert!(e.contains(says), "{url}: {e}");
            assert!(
                !e.contains("sk-secret"),
                "{url}: the refusal echoed it: {e}"
            );
        }
        let e = validate(cfg(json!({
            "standIns": [{ "domain": "x.com", "title": " ", "url": "https://x.com/mcp" }]
        })))
        .unwrap_err();
        assert!(e.contains("title"), "{e}");
        let e = validate(cfg(json!({
            "standIns": [
                { "domain": "x.com", "title": "X", "url": "https://x.com/mcp" },
                { "domain": "X.com", "title": "X2", "url": "https://x.com/mcp2" },
            ]
        })))
        .unwrap_err();
        assert!(e.contains("two stand-ins"), "{e}");
        let many: Vec<String> = (0..=MAX_DOMAINS).map(|i| format!("d{i}.com")).collect();
        assert!(validate(cfg(json!({ "domains": many }))).is_err());
    }

    #[test]
    fn the_stored_shape_and_the_wire_are_camel_case() {
        let c = cfg(json!({
            "standIns": [{ "domain": "x.com", "title": "X", "url": "https://x.com/mcp" }]
        }));
        let stored = serde_json::to_value(&c).unwrap();
        assert!(stored.get("standIns").is_some());
        assert!(stored.get("stand_ins").is_none());
        let w = wire(&c);
        assert_eq!(
            w["builtins"].as_array().unwrap().len(),
            FEATURED_DOMAINS.len()
        );
        assert_eq!(w["shelf"].as_array().unwrap().last().unwrap(), "x.com");
        assert_eq!(w["standIns"][0]["url"], "https://x.com/mcp");
        // An unreadable stored row is "no edits", not an error.
        let junk: FeaturedConfig = serde_json::from_value(json!("junk")).unwrap_or_default();
        assert_eq!(junk, FeaturedConfig::default());
    }
}
