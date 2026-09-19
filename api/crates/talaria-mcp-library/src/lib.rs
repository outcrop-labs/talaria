// The MCP server library — the OFFICIAL MCP registry, queried live and
// parsed per the LATEST server.json schema (2025-12-11: title, websiteUrl,
// themed icons, remote header declarations). Ranked for a BUSINESS platform:
//   first-party  verified reverse-DNS namespace AND the hosted endpoint (or
//                website) lives on that domain — Vercel's own mcp.vercel.com
//   verified     domain-verified namespace, hosted elsewhere
//   community    io.github.* — surfaces only in explicit searches
// A hosted (streamable-http) remote wins; entries shipping only PACKAGES
// (npm/pypi/oci) also appear — the runtime in pkg.rs runs them as hardened
// docker children. The featured shelf and brand pins stay hosted-only.
//
// SERVING STRATEGY — why this module has three layers of caching:
//   1. Bounded concurrency. The registry answers one search in ~100-400ms and
//      THROTTLES bursts: the featured shelf used to resolve all 32 publishers
//      through one unbounded fan-out, half the calls queued for 3-4.6s, and
//      the shelf (which waits for the slowest) took ~4.6s — the "marketplace
//      loads super slowly" report. Six in flight stays in the registry's fast
//      lane: measured 271ms pooled vs 4615ms unbounded.
//   2. Stale-while-revalidate. The featured shelf is editorial — an hour-old
//      answer NOW beats a several-second fan-out for a fresh one. Stale data
//      is served instantly and refreshed in the background, single-flighted.
//   3. A scheduler job warms the shelf at boot and every 30 minutes, so even
//      the one truly cold load (first request ever, registry up) usually
//      happens before any user opens the marketplace.
//
// The state lives inside a constructed Library with the edge, the clock,
// and the icon warm hook injected — `library()` is the production singleton
// wired to safe_fetch and the shared icon cache. The single-flight is a held
// flight lock plus a requested-at stamp (a pass that landed after a caller
// asked is shared with that caller); the scheduled job asks on its own 30min
// cadence, so an older shelf never satisfies it.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use futures_util::future::BoxFuture;
use futures_util::stream::{self, StreamExt};
use serde_json::Value;

use talaria_mcp_icons::IconKey;
use talaria_safe_fetch::{SafeFetch, safe_fetch};
use talaria_scheduler::{JobName, JobSpec};

const REGISTRY_URL: &str = "https://registry.modelcontextprotocol.io/v0/servers";
const CACHE_MS: i64 = 15 * 60 * 1000;
/// Same number the icon warm uses; measured to stay under the registry's
/// burst throttle (see the header). Applied by the `buffered` pool.
const REGISTRY_CONCURRENCY: usize = 6;
const PUBLISHER_TTL_MS: i64 = 60 * 60 * 1000;
const WELL_KNOWN_TTL_MS: i64 = 60 * 60 * 1000;
const FEATURED_FRESH_MS: i64 = 60 * 60 * 1000;

/// Hosting-platform suffixes: a namespace whose reversed domain lands on one
/// of these is a tenant of a shared host (`app.vercel.hobby` →
/// `hobby.vercel.app`), not a domain-verified publisher. Matching its own
/// host does NOT make such a server first-party — that badge belongs to the
/// platform, so the tier demotes to community.
const SHARED_HOSTING_SUFFIXES: &[&str] = &[
    "vercel.app",
    "netlify.app",
    "pages.dev",
    "workers.dev",
    "github.io",
    "gitlab.io",
    "fly.dev",
    "deno.dev",
];

fn on_shared_host(domain: Option<&str>) -> bool {
    domain.is_some_and(|d| {
        SHARED_HOSTING_SUFFIXES
            .iter()
            .any(|s| d == *s || d.ends_with(&format!(".{s}")))
    })
}

/// The featured shelf is EDITORIAL — services businesses actually run on — but
/// the DATA stays live: each name resolves against the registry at request
/// time, and companies that haven't published a server simply don't appear.
/// Pub so anything that asks "is this publisher featured?" reads the one true
/// list instead of a copy that drifts.
pub const FEATURED_DOMAINS: &[&str] = &[
    "github.com",
    "gitlab.com",
    "linear.app",
    "atlassian.com",
    "asana.com",
    "monday.com",
    "notion.so",
    "airtable.com",
    "figma.com",
    "canva.com",
    "slack.com",
    "intercom.com",
    "stripe.com",
    "paypal.com",
    "squareup.com",
    "shopify.com",
    "hubspot.com",
    "vercel.com",
    "netlify.com",
    "cloudflare.com",
    "sentry.io",
    "supabase.com",
    "neon.tech",
    "mongodb.com",
    "elastic.co",
    "twilio.com",
    "zapier.com",
    "huggingface.co",
    "browserbase.com",
    "e2b.dev",
    "postman.com",
    "linkup.so",
];

// Publisher resolution beyond the registry: not every company has registered
// (GitHub hasn't). Two live fallbacks: the /.well-known/mcp.json convention on
// the publisher's domain (Notion serves it: name/description/icon/endpoint),
// and a tiny factual map of DOCUMENTED official endpoints for majors absent
// from both — the exception that keeps GitHub findable, not a catalog.
struct KnownEndpoint {
    title: &'static str,
    url: &'static str,
    description: &'static str,
}

fn known_endpoints() -> &'static HashMap<&'static str, KnownEndpoint> {
    static KNOWN: LazyLock<HashMap<&'static str, KnownEndpoint>> = LazyLock::new(|| {
        let mut m = HashMap::new();
        m.insert(
            "github.com",
            KnownEndpoint {
                title: "GitHub",
                url: "https://api.githubcopilot.com/mcp/",
                description: "GitHub's official MCP server: repos, issues, PRs, actions, via OAuth.",
            },
        );
        m
    });
    &KNOWN
}

/// One entry of a header's `variables` map — an `Input` in the registry
/// schema, carrying the same prompt metadata as a header but for a single
/// `{template}` variable inside its `value`.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryVariable {
    pub description: Option<String>,
    pub is_secret: bool,
    pub placeholder: Option<String>,
    pub default: Option<String>,
    pub choices: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LibraryHeader {
    pub name: String,
    pub description: Option<String>,
    pub is_required: bool,
    pub is_secret: bool,
    pub placeholder: Option<String>,
    pub default: Option<String>,
    pub choices: Option<Vec<String>>,
    /// The registry's `value`: a fixed header, optionally with `{variables}`
    /// the user fills in. None = the header itself is user-supplied.
    pub value: Option<String>,
    pub variables: Option<HashMap<String, LibraryVariable>>,
}

/// Declared order is the ranking: first-party < verified < community.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    FirstParty,
    Verified,
    Community,
}

/// A registry PACKAGE — how the long tail ships: npm/pypi packages or oci
/// images speaking stdio (or an in-container http port). Normalized here so
/// the install POST can store it verbatim; the runtime (pkg.rs) turns it
/// into a hardened `docker run` child.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryPackage {
    /// npm | pypi | oci (the registry's registryType, lowercased).
    pub kind: String,
    pub identifier: String,
    pub version: Option<String>,
    pub runtime_hint: Option<String>,
    /// stdio | http (the registry's streamable-http, normalized).
    pub transport: String,
    /// http packages: the port + path the transport URL names.
    pub container_port: Option<u16>,
    pub transport_path: Option<String>,
    /// oci: the image ref without the tag (spawns pin the digest).
    pub image: String,
    /// Declared run arguments (Input-shaped; named docker flags are
    /// allowlisted at install).
    pub run_args: Vec<Value>,
    /// Declared environment variables — the SAME Input schema as remote
    /// headers, driving the same install form.
    pub declared_env: Vec<LibraryHeader>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LibraryServer {
    /// Registry identity, e.g. "com.vercel/vercel-mcp".
    pub registry_name: String,
    pub title: String,
    pub description: Option<String>,
    pub url: String,
    /// The publisher's domain (websiteUrl host, else reversed namespace).
    pub domain: Option<String>,
    /// Registry-declared icon (latest schema) — hotlinked directly by the UI;
    /// the favicon proxy is only the fallback when this is absent.
    pub icon: Option<String>,
    pub tier: Tier,
    /// Credential/header declarations from the remote — drive the install form.
    pub required_headers: Vec<LibraryHeader>,
    /// Package-shipping entries (no hosted remote): how to run them.
    pub package: Option<LibraryPackage>,
}

// ── The injectable edge ─────────────────────────────────────────────────────

pub struct EdgeRequest {
    pub url: String,
    pub timeout_ms: u64,
    pub max_bytes: u64,
}

pub struct EdgeResponse {
    pub status: u16,
    pub json: Value,
}

pub type FetchEdge =
    Arc<dyn Fn(&EdgeRequest) -> BoxFuture<'static, Result<EdgeResponse, String>> + Send + Sync>;
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;
pub type WarmIcons = Arc<dyn Fn(&[LibraryServer]) + Send + Sync>;

// ── classify ────────────────────────────────────────────────────────────────

fn host_of(u: &str) -> Option<String> {
    reqwest::Url::parse(u)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
}

fn on_domain(h: Option<&str>, domain: &str) -> bool {
    h.map(|h| h == domain || h.ends_with(&format!(".{domain}")))
        .unwrap_or(false)
}

fn strip_brand_label(d: &str) -> &str {
    // One leading label only — www/mcp/docs/support/api.
    for label in ["www", "mcp", "docs", "support", "api"] {
        if let Some(rest) = d.strip_prefix(&format!("{label}.")) {
            return rest;
        }
    }
    d
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// One `variables` entry — an `Input`, sharing the prompt metadata of a
/// header minus the header-only fields (name, isRequired).
fn parse_variable(v: &Value) -> LibraryVariable {
    LibraryVariable {
        description: v
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        is_secret: v.get("isSecret").and_then(Value::as_bool).unwrap_or(false),
        placeholder: v
            .get("placeholder")
            .and_then(Value::as_str)
            .map(str::to_string),
        default: v.get("default").and_then(Value::as_str).map(str::to_string),
        choices: v.get("choices").and_then(Value::as_array).map(|c| {
            c.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        }),
    }
}

fn is_generic_title(title: &str) -> bool {
    // Generic noise, case-insensitive.
    let t = title.to_lowercase();
    matches!(
        t.as_str(),
        "mcp" | "mcp-server" | "mcp_server" | "mcpserver" | "server"
    )
}

fn is_lone_lowercase_word(title: &str) -> bool {
    // First char a lowercase letter, rest lowercase/digit — "linear" →
    // "Linear": lone lowercase words present as brands.
    let mut chars = title.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// One Input-shaped declaration list (remote headers, package env) → the
/// prompt metadata the install form renders.
fn parse_declared(list: Option<&Value>) -> Vec<LibraryHeader> {
    list.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|h| {
                    Some(LibraryHeader {
                        name: h.get("name")?.as_str()?.to_string(),
                        description: h
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        is_required: h
                            .get("isRequired")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        is_secret: h.get("isSecret").and_then(Value::as_bool).unwrap_or(false),
                        placeholder: h
                            .get("placeholder")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        default: h.get("default").and_then(Value::as_str).map(str::to_string),
                        choices: h.get("choices").and_then(Value::as_array).map(|c| {
                            c.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        }),
                        value: h.get("value").and_then(Value::as_str).map(str::to_string),
                        variables: h.get("variables").and_then(Value::as_object).map(|m| {
                            m.iter()
                                .map(|(k, v)| (k.clone(), parse_variable(v)))
                                .collect()
                        }),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The runnable package from an entry's `packages` array — preference order
/// oci (self-contained) → pypi → npm; nuget and anything else has no runtime
/// here, which is a drop. Pure.
fn classify_package(server: &Value) -> Option<LibraryPackage> {
    let packages = server.get("packages").and_then(Value::as_array)?;
    let pick = |kinds: &[&str]| {
        packages.iter().find(|p| {
            p.get("registryType")
                .and_then(Value::as_str)
                .map(str::to_lowercase)
                .is_some_and(|r| kinds.contains(&r.as_str()))
        })
    };
    let pkg = pick(&["oci"])
        .or_else(|| pick(&["pypi"]))
        .or_else(|| pick(&["npm"]))?;
    let kind = pkg.get("registryType")?.as_str()?.to_lowercase();
    let identifier = pkg.get("identifier")?.as_str()?.to_string();
    let version = pkg
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_string);
    let runtime_hint = pkg
        .get("runtimeHint")
        .and_then(Value::as_str)
        .map(str::to_string);
    let transport_v = pkg.get("transport").unwrap_or(&Value::Null);
    let ttype = transport_v
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("stdio");
    let transport = if matches!(ttype, "streamable-http" | "sse") {
        "http"
    } else {
        "stdio"
    };
    // An oci identifier may carry its tag — the image ref is what's left.
    let (image, version) = if kind == "oci" {
        match identifier.rsplit_once(':') {
            Some((img, tag)) if !tag.contains('/') => (img.to_string(), Some(tag.to_string())),
            _ => (identifier.clone(), version),
        }
    } else {
        (String::new(), version)
    };
    let (container_port, transport_path) = transport_v
        .get("url")
        .and_then(Value::as_str)
        .and_then(|u| reqwest::Url::parse(u).ok())
        .map(|u| (u.port(), Some(u.path().to_string())))
        .unwrap_or((None, None));
    let run_args = pkg
        .get("runtimeArguments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let declared_env = parse_declared(pkg.get("environmentVariables"));
    Some(LibraryPackage {
        kind,
        identifier,
        version,
        runtime_hint,
        transport: transport.to_string(),
        container_port,
        transport_path,
        image,
        run_args,
        declared_env,
    })
}

/// One registry entry → one shelf entry, or None to drop it (not official-
/// active, nothing installable). A hosted https remote wins; otherwise a
/// runnable package keeps the entry. Pure — the tests drive it directly.
fn classify(e: &Value) -> Option<LibraryServer> {
    let server = e.get("server")?;
    let name = server.get("name")?.as_str()?;
    if let Some(official) = e
        .get("_meta")
        .and_then(|m| m.get("io.modelcontextprotocol.registry/official"))
    {
        let active = official.get("status").and_then(Value::as_str) == Some("active");
        let latest = official.get("isLatest").and_then(Value::as_bool);
        if !active || latest == Some(false) {
            return None;
        }
    }
    let remotes = server.get("remotes").and_then(Value::as_array);
    let remote = remotes.into_iter().flatten().find(|x| {
        x.get("type").and_then(Value::as_str) == Some("streamable-http")
            && x.get("url")
                .and_then(Value::as_str)
                .is_some_and(|u| u.starts_with("https://"))
    });
    let package = if remote.is_none() {
        Some(classify_package(server)?)
    } else {
        None
    };
    let remote_headers = remote.as_ref().and_then(|r| r.get("headers"));
    let remote_url = remote
        .and_then(|r| r.get("url"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    let ns = name.split('/').next().unwrap_or("");
    let community = ns.starts_with("io.github.");
    let ns_domain: Option<String> =
        (!community).then(|| ns.split('.').rev().collect::<Vec<_>>().join("."));
    let website = server.get("websiteUrl").and_then(Value::as_str);
    let domain = host_of(website.unwrap_or("")).or(ns_domain.clone());
    let mut tier = if community {
        Tier::Community
    } else {
        Tier::Verified
    };
    let remote_on_ns = ns_domain
        .as_deref()
        .is_some_and(|d| on_domain(host_of(&remote_url).as_deref(), d));
    let site_on_ns = ns_domain
        .as_deref()
        .is_some_and(|d| on_domain(host_of(website.unwrap_or("")).as_deref(), d));
    if ns_domain.is_some() && (remote_on_ns || site_on_ns) {
        tier = Tier::FirstParty;
    }
    // A shared-host tenant matching its own *.vercel.app host is not the
    // platform's official server — the demote runs after the promotion
    // because it wins.
    if on_shared_host(ns_domain.as_deref()) {
        tier = Tier::Community;
    }
    // Themed icons: prefer an untinted/light entry with an https src.
    let icons: Vec<&Value> = server
        .get("icons")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|i| {
                    i.get("src")
                        .and_then(Value::as_str)
                        .is_some_and(|src| src.starts_with("https://"))
                })
                .collect()
        })
        .unwrap_or_default();
    let icon = icons
        .iter()
        .find(|i| {
            let theme = i.get("theme").and_then(Value::as_str);
            theme.is_none() || theme == Some("light")
        })
        .or_else(|| icons.first())
        .and_then(|i| i.get("src").and_then(Value::as_str))
        .map(str::to_string);
    // Generic titles ("mcp", "mcp-server") read as noise — brand from the
    // publisher's domain instead: stripe.com → Stripe.
    let display_domain = ns_domain
        .clone()
        .or(domain.clone())
        .map(|d| strip_brand_label(&d).to_string());
    let mut title = server
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| name.rsplit('/').next().unwrap_or(name).to_string());
    if is_generic_title(&title)
        && let Some(d) = &display_domain
    {
        let brand = d.split('.').next().unwrap_or(d);
        title = capitalize_first(brand);
    }
    // "linear" → "Linear": lone lowercase words present as brands.
    if is_lone_lowercase_word(&title) {
        title = capitalize_first(&title);
    }
    let description = server
        .get("description")
        .and_then(Value::as_str)
        .map(|d| d.chars().take(220).collect::<String>());
    let required_headers = parse_declared(remote_headers);
    Some(LibraryServer {
        registry_name: name.to_string(),
        title,
        description,
        url: remote_url,
        domain: display_domain,
        icon,
        tier,
        required_headers,
        package,
    })
}

// ── The library ─────────────────────────────────────────────────────────────

/// A cached search answer: when it landed, and the shared shelf.
type SearchShelf = HashMap<String, (i64, Arc<Vec<LibraryServer>>)>;
/// A cached per-domain resolution, misses included (None = known-absent).
type PublisherShelf = HashMap<String, (i64, Option<Arc<LibraryServer>>)>;

pub struct Library {
    edge: FetchEdge,
    warm: WarmIcons,
    now: NowFn,
    search_cache: Mutex<SearchShelf>,
    well_known_cache: Mutex<PublisherShelf>,
    publisher_cache: Mutex<PublisherShelf>,
    featured: Mutex<Option<(i64, Arc<Vec<LibraryServer>>)>>,
    /// The single-flight. Held across a whole fan-out — that is its job.
    flight: tokio::sync::Mutex<()>,
}

impl Library {
    pub fn new(edge: FetchEdge, warm: WarmIcons, now: NowFn) -> Library {
        Library {
            edge,
            warm,
            now,
            search_cache: Mutex::new(HashMap::new()),
            well_known_cache: Mutex::new(HashMap::new()),
            publisher_cache: Mutex::new(HashMap::new()),
            featured: Mutex::new(None),
            flight: tokio::sync::Mutex::new(()),
        }
    }

    fn now(&self) -> i64 {
        (self.now)()
    }

    fn featured_snapshot(&self) -> Option<(i64, Arc<Vec<LibraryServer>>)> {
        self.featured
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    async fn edge(
        &self,
        url: &str,
        timeout_ms: u64,
        max_bytes: u64,
    ) -> Result<EdgeResponse, String> {
        let req = EdgeRequest {
            url: url.to_string(),
            timeout_ms,
            max_bytes,
        };
        (self.edge)(&req).await
    }

    /// One registry page. Non-OK statuses error as `registry <status>`; a
    /// bad body errors too — resolvePublisher treats both as "registry
    /// hiccup, try the fallbacks".
    async fn registry_page(&self, params: &[(&str, &str)]) -> Result<Vec<Value>, String> {
        let mut url = reqwest::Url::parse(REGISTRY_URL).map_err(|e| e.to_string())?;
        {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in params {
                pairs.append_pair(k, v);
            }
        }
        // Third-party feed — through the SSRF guard.
        let r = self.edge(url.as_str(), 10_000, 5 * 1024 * 1024).await?;
        if !(200..300).contains(&r.status) {
            return Err(format!("registry {}", r.status));
        }
        Ok(r.json
            .get("servers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// The /.well-known/mcp.json convention on the publisher's domain. Cached
    /// an hour, misses included — a domain without one stays cheap.
    async fn well_known_server(&self, domain: &str) -> Option<Arc<LibraryServer>> {
        if let Some((at, hit)) = self
            .well_known_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(domain)
            .cloned()
            && self.now() - at < WELL_KNOWN_TTL_MS
        {
            return hit;
        }
        let mut server = None;
        if let Ok(r) = self
            .edge(
                &format!("https://{domain}/.well-known/mcp.json"),
                5_000,
                256 * 1024,
            )
            .await
            && (200..300).contains(&r.status)
        {
            let endpoint = r.json.get("endpoint").and_then(Value::as_str);
            if endpoint.is_some_and(|e| e.starts_with("https://")) {
                server = Some(Arc::new(LibraryServer {
                    registry_name: format!("wk:{domain}"),
                    title: r
                        .json
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| domain.split('.').next().unwrap_or(domain).to_string()),
                    description: r
                        .json
                        .get("description")
                        .and_then(Value::as_str)
                        .map(|d| d.chars().take(220).collect::<String>()),
                    url: endpoint.unwrap().to_string(),
                    domain: Some(domain.to_string()),
                    icon: r
                        .json
                        .get("icon")
                        .and_then(Value::as_str)
                        .filter(|i| i.starts_with("https://"))
                        .map(str::to_string),
                    tier: Tier::FirstParty,
                    required_headers: Vec::new(),
                    package: None,
                }));
            }
        }
        self.well_known_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(domain.to_string(), (self.now(), server.clone()));
        server
    }

    fn known_server(domain: &str) -> Option<Arc<LibraryServer>> {
        known_endpoints().get(domain).map(|k| {
            Arc::new(LibraryServer {
                registry_name: format!("known:{domain}"),
                title: k.title.to_string(),
                description: Some(k.description.to_string()),
                url: k.url.to_string(),
                domain: Some(domain.to_string()),
                icon: None,
                tier: Tier::FirstParty,
                required_headers: Vec::new(),
                package: None,
            })
        })
    }

    /// One publisher, best source wins: registry → well-known → documented.
    /// Resolved at most once an hour per domain, and the featured refresh warms
    /// every FEATURED_DOMAIN — so a brand-shaped search ("git", "stripe") pins
    /// its publisher from cache instead of re-resolving it live.
    ///
    /// Takes an owned domain on purpose: an async fn borrowing its argument
    /// makes the pool's `.map` closure higher-ranked over that lifetime, and
    /// the spawned refresh then fails the 'static check with rustc's
    /// "FnOnce is not general enough".
    async fn resolve_publisher(&self, domain: String) -> Option<Arc<LibraryServer>> {
        let domain = &*domain;
        if let Some((at, hit)) = self
            .publisher_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(domain)
            .cloned()
            && self.now() - at < PUBLISHER_TTL_MS
        {
            return hit;
        }
        let term = domain.split('.').next().unwrap_or(domain);
        let mut server = None;
        if let Ok(entries) = self
            .registry_page(&[("search", term), ("limit", "30")])
            .await
        {
            let mut hits: Vec<LibraryServer> = entries
                .iter()
                .filter_map(classify)
                .filter(|s| {
                    s.package.is_none()
                        && (s.domain.as_deref() == Some(domain)
                            || on_domain(s.domain.as_deref(), domain))
                })
                .collect();
            hits.sort_by_key(|s| s.tier);
            server = hits.into_iter().next().map(Arc::new);
        }
        if server.is_none() {
            server = self
                .well_known_server(domain)
                .await
                .or_else(|| Self::known_server(domain));
        }
        self.publisher_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(domain.to_string(), (self.now(), server.clone()));
        server
    }

    /// Resolve the featured shelf from live sources and cache the result.
    /// Never fails for per-domain failures — a publisher that cannot be
    /// resolved just drops off the shelf until it can be.
    async fn refresh_featured(&self) -> Arc<Vec<LibraryServer>> {
        // Owned `String` items into the pool, on purpose: any reference
        // crossing the `.map` closure boundary makes the closure
        // higher-ranked over that lifetime, and the spawned refresh then
        // fails the 'static check with rustc's "FnOnce is not general
        // enough". 32 allocations per fan-out is nothing.
        let domains: Vec<String> = FEATURED_DOMAINS.iter().map(|d| d.to_string()).collect();
        let servers: Vec<LibraryServer> = stream::iter(domains)
            .map(|d| self.resolve_publisher(d))
            .buffered(REGISTRY_CONCURRENCY)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .flatten()
            .map(|s| (*s).clone())
            .collect();
        let servers = Arc::new(servers);
        // Only a NON-EMPTY answer replaces what's cached: an empty or partial
        // one means the registry is misbehaving, and evicting a good shelf for
        // it would trade a stale shelf for an empty one.
        if !servers.is_empty() {
            *self.featured.lock().unwrap_or_else(|p| p.into_inner()) =
                Some((self.now(), servers.clone()));
        }
        (self.warm)(&servers);
        servers
    }

    /// Single-flight the refresh so N concurrent stale serves trigger ONE
    /// fan-out, not N. A pass that landed at-or-after this caller asked is
    /// shared with it (`>=`, not `>`, because a pass landing in the same
    /// millisecond the caller asked is still that pass's answer); the
    /// scheduled job asks on its own 30min cadence, so a shelf that predates
    /// its request never satisfies it.
    pub async fn refresh_featured_once(&self) -> Arc<Vec<LibraryServer>> {
        let requested_at = self.now();
        let _flight = self.flight.lock().await;
        if let Some((at, servers)) = self.featured_snapshot()
            && at >= requested_at
        {
            return servers;
        }
        self.refresh_featured().await
    }

    /// Stale-while-revalidate: serve the shelf we have, however old, and only
    /// then kick a refresh. The only caller who ever blocks on the fan-out is
    /// the one who arrives before ANY answer exists — in production the
    /// scheduler job below has usually been there first.
    pub async fn featured(self: &Arc<Self>) -> Vec<LibraryServer> {
        if let Some((at, servers)) = self.featured_snapshot() {
            if self.now() - at >= FEATURED_FRESH_MS {
                let me = Arc::clone(self);
                tokio::spawn(async move {
                    let _ = me.refresh_featured_once().await;
                });
            }
            return servers.iter().cloned().collect();
        }
        self.refresh_featured_once().await.iter().cloned().collect()
    }

    /// Search the registry SERVER-SIDE (complete over 20k+ entries), rank
    /// locally: first-party publishers top the results, community wrappers
    /// sink. Favicon fallbacks warm in the background as results are served.
    pub async fn search(&self, query: &str) -> Vec<LibraryServer> {
        let q = query.trim().to_lowercase();
        if let Some((at, hit)) = self
            .search_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&q)
            .cloned()
            && self.now() - at < CACHE_MS
        {
            return hit.iter().cloned().collect();
        }
        let entries = if q.is_empty() {
            self.registry_page(&[("limit", "100")]).await
        } else {
            self.registry_page(&[("search", &q), ("limit", "100")])
                .await
        }
        .unwrap_or_default();
        let mut by_name: HashMap<String, LibraryServer> = HashMap::new();
        for e in &entries {
            if let Some(s) = classify(e) {
                by_name.insert(s.registry_name.clone(), s);
            }
        }
        // Tier, then title — byte order on the (ASCII-lowercase-heavy)
        // titles, not locale-aware; ordering inside a tier is presentation,
        // not contract.
        let mut results: Vec<LibraryServer> = by_name.into_values().collect();
        results.sort_by(|a, b| a.tier.cmp(&b.tier).then_with(|| a.title.cmp(&b.title)));
        results.truncate(40);
        // A brand-shaped query ("github", "notion") pins the publisher's
        // OFFICIAL server on top — resolved through the full chain, so a
        // company the registry lacks (GitHub) still lands first instead of
        // wrapper noise.
        let brandy = !q.is_empty()
            && q.len() >= 2
            && q.len() <= 20
            && q.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == ' ');
        if brandy {
            let squashed = q.replace(' ', "");
            let mut brands: Vec<&str> = FEATURED_DOMAINS.to_vec();
            for key in known_endpoints().keys() {
                if !brands.contains(key) {
                    brands.push(key);
                }
            }
            let brands: Vec<&str> = brands
                .into_iter()
                .filter(|d| d.split('.').next().unwrap_or(d).starts_with(&squashed))
                .take(3)
                .collect();
            let pinned: Vec<Arc<LibraryServer>> = futures_util::future::join_all(
                brands
                    .iter()
                    .map(|d| self.resolve_publisher((*d).to_string())),
            )
            .await
            .into_iter()
            .flatten()
            .collect();
            let pinned_urls: std::collections::HashSet<&str> =
                pinned.iter().map(|s| s.url.as_str()).collect();
            let mut merged: Vec<LibraryServer> = pinned.iter().map(|s| (**s).clone()).collect();
            merged.extend(
                results
                    .iter()
                    .filter(|s| !pinned_urls.contains(s.url.as_str()))
                    .cloned(),
            );
            merged.truncate(40);
            results = merged;
        }
        let results = Arc::new(results);
        self.search_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(q, (self.now(), results.clone()));
        (self.warm)(&results);
        results.iter().cloned().collect()
    }
}

// ── Production wiring + the scheduled job ───────────────────────────────────

fn real_edge() -> FetchEdge {
    Arc::new(|req: &EdgeRequest| {
        let url = req.url.clone();
        let opts = SafeFetch {
            timeout_ms: Some(req.timeout_ms),
            max_bytes: Some(req.max_bytes),
            ..Default::default()
        };
        Box::pin(async move {
            let resp = safe_fetch(&url, opts).await.map_err(|e| e.to_string())?;
            let json = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
            Ok(EdgeResponse {
                status: resp.status,
                json,
            })
        })
    })
}

fn real_warm() -> WarmIcons {
    Arc::new(|servers: &[LibraryServer]| {
        let keys: Vec<IconKey> = servers
            .iter()
            .filter(|s| s.icon.is_none() && s.domain.is_some())
            .map(|s| IconKey {
                src: None,
                domain: s.domain.clone(),
            })
            .collect();
        if !keys.is_empty() {
            talaria_mcp_icons::icons().warm(keys);
        }
    })
}

/// The production singleton — safe_fetch edge, wall clock, shared icon cache.
pub fn library() -> Arc<Library> {
    static LIB: LazyLock<Arc<Library>> = LazyLock::new(|| {
        let now: NowFn = Arc::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0)
        });
        Arc::new(Library::new(real_edge(), real_warm(), now))
    });
    Arc::clone(&LIB)
}

pub const MCP_REFRESH_EVERY_MS: u64 = 30 * 60_000;
pub const MCP_REFRESH_FIRST_RUN_DELAY_MS: u64 = 10_000;
pub const MCP_REFRESH_MAX_RUN_MS: u64 = 2 * 60_000;

pub fn mcp_library_refresh_job_spec(lib: Arc<Library>) -> JobSpec {
    JobSpec {
        name: JobName::McpLibraryRefresh,
        every_ms: MCP_REFRESH_EVERY_MS,
        // Early: it warms a cache, writes nothing anyone sees, and the whole
        // point is to be done before the first user opens the marketplace.
        first_run_delay_ms: Some(MCP_REFRESH_FIRST_RUN_DELAY_MS),
        max_run_ms: Some(MCP_REFRESH_MAX_RUN_MS),
        // perInstance because the cache being warmed is THIS process's
        // memory; the upstream work is read-only, so instances duplicating it
        // is the intended behavior.
        per_instance: true,
        run: Arc::new(move || {
            let lib = lib.clone();
            Box::pin(async move {
                let servers = lib.refresh_featured_once().await;
                Ok(Some(format!(
                    "{} featured marketplace server(s)",
                    servers.len()
                )))
            })
        }),
    }
}

pub fn register_mcp_library_refresh_job(lib: Arc<Library>) {
    talaria_scheduler::register_job(mcp_library_refresh_job_spec(lib));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── the faked edge ──────────────────────────────────────────────────────

    type RegistryFn = Box<dyn Fn(&str) -> Vec<Value> + Send + Sync>;
    type WellKnownFn = Box<dyn Fn(&str) -> Option<Value> + Send + Sync>;

    struct FakeState {
        calls: Vec<String>,
        registry_in_flight: usize,
        registry_max_in_flight: usize,
        registry: RegistryFn,
        well_known: WellKnownFn,
        /// When set, every fetch parks here until released — for proving a
        /// caller did NOT wait for the network.
        gate: Option<Vec<tokio::sync::oneshot::Sender<()>>>,
    }

    type Shared = Arc<Mutex<FakeState>>;

    /// A first-party registry entry for a domain, description overridable (the
    /// stale-while-revalidate test flips it OLD → NEW).
    fn entry_for(domain: &str, description: &str) -> Value {
        let labels: Vec<&str> = domain.split('.').collect();
        let ns = labels.iter().rev().copied().collect::<Vec<_>>().join(".");
        json!({
            "server": {
                "name": format!("{ns}/{}-mcp", labels[0]),
                "title": labels[0],
                "description": description,
                "websiteUrl": format!("https://{domain}"),
                "remotes": [{ "type": "streamable-http", "url": format!("https://mcp.{domain}/mcp") }],
            }
        })
    }

    /// A registry where every featured domain has a first-party entry except
    /// the misses given — those exercise the well-known → documented fallbacks.
    /// Answers PER TERM (like the real registry's search), so a miss returns
    /// an empty page, not everyone else's entries.
    fn registry_with(featured: &[&str], misses: &[&str], flavor: &str) -> RegistryFn {
        let flavor = flavor.to_string();
        let by_term: HashMap<String, String> = featured
            .iter()
            .filter(|d| !misses.contains(d))
            .map(|d| (d.split('.').next().unwrap().to_string(), d.to_string()))
            .collect();
        Box::new(move |term| {
            by_term
                .get(term)
                .map(|d| vec![entry_for(d, &format!("{flavor} {d}"))])
                .unwrap_or_default()
        })
    }

    fn fake_edge(state: Shared) -> FetchEdge {
        Arc::new(move |req: &EdgeRequest| {
            let state = state.clone();
            let url = req.url.clone();
            let term = reqwest::Url::parse(&url)
                .ok()
                .and_then(|u| {
                    u.query_pairs()
                        .find(|(k, _)| k == "search")
                        .map(|(_, v)| v.to_string())
                })
                .unwrap_or_default();
            Box::pin(async move {
                let is_registry = url.starts_with("https://registry.modelcontextprotocol.io/");
                let label = if is_registry {
                    format!("registry.modelcontextprotocol.io/v0/servers?{term}")
                } else {
                    url.clone()
                };
                {
                    let mut s = state.lock().unwrap();
                    s.calls.push(label);
                    if is_registry {
                        s.registry_in_flight += 1;
                        s.registry_max_in_flight =
                            s.registry_max_in_flight.max(s.registry_in_flight);
                    }
                }
                let answer = async {
                    if let Some(receiver) = {
                        let (tx, rx) = tokio::sync::oneshot::channel();
                        let mut s = state.lock().unwrap();
                        match &mut s.gate {
                            Some(gate) => {
                                gate.push(tx);
                                Some(rx)
                            }
                            None => None,
                        }
                    } {
                        let _ = receiver.await;
                    }
                    for _ in 0..4 {
                        tokio::task::yield_now().await;
                    }
                    if is_registry {
                        let s = state.lock().unwrap();
                        let servers = (s.registry)(&term);
                        Ok(EdgeResponse {
                            status: 200,
                            json: json!({ "servers": servers }),
                        })
                    } else {
                        let domain = url
                            .strip_prefix("https://")
                            .and_then(|r| r.split('/').next())
                            .unwrap_or("");
                        let s = state.lock().unwrap();
                        match (s.well_known)(domain) {
                            Some(body) => Ok(EdgeResponse {
                                status: 200,
                                json: body,
                            }),
                            None => Ok(EdgeResponse {
                                status: 404,
                                json: json!({}),
                            }),
                        }
                    }
                }
                .await;
                if is_registry {
                    state.lock().unwrap().registry_in_flight -= 1;
                }
                answer
            })
        })
    }

    fn state_with(registry: RegistryFn) -> Shared {
        Arc::new(Mutex::new(FakeState {
            calls: Vec::new(),
            registry_in_flight: 0,
            registry_max_in_flight: 0,
            registry,
            well_known: Box::new(|_| None),
            gate: None,
        }))
    }

    fn clock() -> (Arc<Mutex<i64>>, NowFn) {
        let t = Arc::new(Mutex::new(1_000_000i64));
        let now: NowFn = {
            let t = t.clone();
            Arc::new(move || *t.lock().unwrap_or_else(|p| p.into_inner()))
        };
        (t, now)
    }

    fn noop_warm() -> WarmIcons {
        Arc::new(|_| {})
    }

    // ── classify, pure ─────────────────────────────────────────────────────

    #[test]
    fn classify_drops_non_active_officials_and_unhosted_entries() {
        let e = json!({
            "server": { "name": "com.acme/acme", "remotes": [{ "type": "stdio", "url": "x" }] }
        });
        assert!(classify(&e).is_none()); // no streamable-http remote
        let e = json!({
            "server": { "name": "com.acme/acme",
                        "remotes": [{ "type": "streamable-http", "url": "http://insecure" }] }
        });
        assert!(classify(&e).is_none()); // http, not https
        let e = json!({
            "_meta": { "io.modelcontextprotocol.registry/official": { "status": "deleted" } },
            "server": { "name": "com.acme/acme",
                        "remotes": [{ "type": "streamable-http", "url": "https://mcp.acme.com/mcp" }] }
        });
        assert!(classify(&e).is_none()); // not active
    }

    #[test]
    fn classify_ranks_first_party_and_brands_generic_titles() {
        let e = json!({
            "server": {
                "name": "com.stripe/stripe-mcp",
                "title": "mcp-server",
                "websiteUrl": "https://stripe.com",
                "remotes": [{ "type": "streamable-http", "url": "https://mcp.stripe.com/mcp" }],
            }
        });
        let s = classify(&e).unwrap();
        assert_eq!(s.tier, Tier::FirstParty);
        assert_eq!(s.title, "Stripe"); // branded from the domain
        assert_eq!(s.domain.as_deref(), Some("stripe.com"));

        // Hosted elsewhere → verified, and lone lowercase words get capitalized.
        let e = json!({
            "server": {
                "name": "com.acme/linear",
                "title": "linear",
                "remotes": [{ "type": "streamable-http", "url": "https://relay.example.com/mcp" }],
            }
        });
        let s = classify(&e).unwrap();
        assert_eq!(s.tier, Tier::Verified);
        assert_eq!(s.title, "Linear");

        // io.github.* is community and never gets a reverse-DNS domain.
        let e = json!({
            "server": {
                "name": "io.github.someone/wrapper",
                "title": "Wrapper",
                "remotes": [{ "type": "streamable-http", "url": "https://relay.example.com/mcp" }],
            }
        });
        let s = classify(&e).unwrap();
        assert_eq!(s.tier, Tier::Community);
        assert_eq!(s.domain, None);
    }

    #[test]
    fn classify_prefers_light_icons_and_maps_header_declarations() {
        let e = json!({
            "server": {
                "name": "com.acme/acme",
                "title": "Acme",
                "icons": [
                    { "src": "http://insecure/x.png" },
                    { "src": "https://acme.com/dark.png", "theme": "dark" },
                    { "src": "https://acme.com/light.png", "theme": "light" },
                ],
                "remotes": [{
                    "type": "streamable-http", "url": "https://mcp.acme.com/mcp",
                    "headers": [{ "name": "X-Key", "description": "the key", "isRequired": true, "choices": ["a", "b"] }],
                }],
            }
        });
        let s = classify(&e).unwrap();
        assert_eq!(s.icon.as_deref(), Some("https://acme.com/light.png"));
        assert_eq!(s.required_headers.len(), 1);
        let h = &s.required_headers[0];
        assert_eq!(h.name, "X-Key");
        assert!(h.is_required);
        assert!(!h.is_secret);
        assert_eq!(
            h.choices.as_deref(),
            Some(&["a".to_string(), "b".to_string()][..])
        );
    }

    #[test]
    fn classify_maps_value_templates_and_variable_metadata() {
        // The registry's DOMINANT declaration style: a fixed `value` with an
        // inline {var}, plus (optionally) a variables map describing it.
        let e = json!({
            "server": {
                "name": "ai.smithery/smithery-notion",
                "title": "Smithery Notion",
                "remotes": [{
                    "type": "streamable-http", "url": "https://server.smithery.ai/@smithery/notion/mcp",
                    "headers": [{
                        "name": "Authorization",
                        "value": "Bearer {smithery_api_key}",
                        "isRequired": true,
                        "isSecret": true,
                        "description": "Bearer token for Smithery authentication",
                        "variables": {
                            "smithery_api_key": { "placeholder": "sk-…", "isSecret": true },
                            "unused_var": { "description": "not referenced by the value" },
                        },
                    }],
                }],
            }
        });
        let s = classify(&e).unwrap();
        let h = &s.required_headers[0];
        assert_eq!(h.value.as_deref(), Some("Bearer {smithery_api_key}"));
        let vars = h.variables.as_ref().unwrap();
        assert_eq!(
            vars.get("smithery_api_key").unwrap().placeholder.as_deref(),
            Some("sk-…")
        );
        assert!(vars.get("unused_var").is_some()); // kept; the UI shows what the template uses
        // A literal value (no braces) is a fixed header, not a prompt.
        let e = json!({
            "server": {
                "name": "com.acme/acme", "title": "Acme",
                "remotes": [{
                    "type": "streamable-http", "url": "https://mcp.acme.com/mcp",
                    "headers": [{ "name": "X-Api-Version", "value": "2" }],
                }],
            }
        });
        let s = classify(&e).unwrap();
        assert_eq!(s.required_headers[0].value.as_deref(), Some("2"));
        assert!(s.required_headers[0].variables.is_none());
        // No value at all = the header itself is user-supplied (today's shape).
        let e = json!({
            "server": {
                "name": "com.acme/acme2", "title": "Acme",
                "remotes": [{
                    "type": "streamable-http", "url": "https://mcp.acme.com/mcp",
                    "headers": [{ "name": "X-Key", "isRequired": true }],
                }],
            }
        });
        let s = classify(&e).unwrap();
        assert_eq!(s.required_headers[0].value, None);
    }

    #[test]
    fn classify_demotes_shared_host_tenants_to_community() {
        // app.vercel.<project> reverses to <project>.vercel.app; the remote
        // lives on that same host, which would read as first-party. It is a
        // tenant of a shared host, not the platform's official server.
        let e = json!({
            "server": {
                "name": "app.vercel.hobby-project/wrapper",
                "title": "Wrapper",
                "remotes": [{ "type": "streamable-http", "url": "https://hobby-project.vercel.app/api/mcp" }],
            }
        });
        let s = classify(&e).unwrap();
        assert_eq!(s.tier, Tier::Community);
        // A real domain-verified publisher keeps its tier.
        let e = json!({
            "server": {
                "name": "com.vercel/vercel-mcp",
                "title": "Vercel",
                "remotes": [{ "type": "streamable-http", "url": "https://mcp.vercel.com" }],
            }
        });
        let s = classify(&e).unwrap();
        assert_eq!(s.tier, Tier::FirstParty);
    }

    #[test]
    fn classify_keeps_package_only_entries_with_normalized_packages() {
        // The io.github long tail: an npm stdio package with declared env in
        // the same Input schema as remote headers.
        let e = json!({
            "server": {
                "name": "io.github.Eszetael/postgres-mcp-hardened",
                "title": "postgres-mcp-hardened",
                "packages": [{
                    "registryType": "npm",
                    "identifier": "postgres-mcp-hardened",
                    "version": "0.1.10",
                    "runtimeHint": "npx",
                    "transport": { "type": "stdio" },
                    "runtimeArguments": [{ "type": "positional", "value": "--stdio" }],
                    "environmentVariables": [{
                        "name": "DATABASE_URL",
                        "description": "Connection string",
                        "isRequired": true,
                        "isSecret": true,
                    }],
                }],
            }
        });
        let s = classify(&e).unwrap();
        assert_eq!(s.tier, Tier::Community);
        let p = s.package.as_ref().expect("the package rides the entry");
        assert_eq!(p.kind, "npm");
        assert_eq!(p.identifier, "postgres-mcp-hardened");
        assert_eq!(p.transport, "stdio");
        assert_eq!(p.declared_env.len(), 1);
        assert!(p.declared_env[0].is_secret);
        assert_eq!(p.run_args.len(), 1);
        assert_eq!(s.url, ""); // no hosted remote — the package IS the install

        // An oci image declaring an http transport: tag splits off the
        // identifier, the port/path come from the transport URL.
        let e = json!({
            "server": {
                "name": "io.github.containers/kubernetes-mcp-server",
                "title": "Kubernetes MCP",
                "packages": [{
                    "registryType": "oci",
                    "identifier": "ghcr.io/containers/kubernetes-mcp-server:v0.0.59",
                    "runtimeHint": "docker",
                    "transport": { "type": "streamable-http", "url": "http://localhost:8080/mcp" },
                }],
            }
        });
        let s = classify(&e).unwrap();
        let p = s.package.as_ref().unwrap();
        assert_eq!(p.kind, "oci");
        assert_eq!(p.image, "ghcr.io/containers/kubernetes-mcp-server");
        assert_eq!(p.version.as_deref(), Some("v0.0.59"));
        assert_eq!(p.transport, "http");
        assert_eq!(p.container_port, Some(8080));
        assert_eq!(p.transport_path.as_deref(), Some("/mcp"));
    }

    #[test]
    fn classify_drops_unrunnable_packages_and_prefers_hosted() {
        // nuget (and anything without a runtime here) is a drop.
        let e = json!({
            "server": {
                "name": "com.example/nuget-only",
                "title": "Nuget Only",
                "packages": [{ "registryType": "nuget", "identifier": "Example.Server" }],
            }
        });
        assert!(classify(&e).is_none());
        // A hosted remote beats a package — no runtime cost.
        let e = json!({
            "server": {
                "name": "com.example/both",
                "title": "Both",
                "remotes": [{ "type": "streamable-http", "url": "https://mcp.example.com/mcp" }],
                "packages": [{ "registryType": "pypi", "identifier": "example", "version": "1.0" }],
            }
        });
        let s = classify(&e).unwrap();
        assert!(s.package.is_none());
        assert_eq!(s.url, "https://mcp.example.com/mcp");
        // Preference when several runtimes ship: oci → pypi → npm.
        let e = json!({
            "server": {
                "name": "io.github.x/multi",
                "title": "Multi",
                "packages": [
                    { "registryType": "npm", "identifier": "x" },
                    { "registryType": "pypi", "identifier": "x" },
                    { "registryType": "oci", "identifier": "ghcr.io/x/y:1" },
                ],
            }
        });
        assert_eq!(classify(&e).unwrap().package.unwrap().kind, "oci");
    }

    // ── the module-boundary tests ───────────────────────────────────────────

    #[tokio::test]
    async fn resolves_the_featured_shelf_with_bounded_registry_concurrency() {
        let registry = registry_with(FEATURED_DOMAINS, &["github.com"], "OLD");
        let state = state_with(registry);
        let (_t, now) = clock();
        let lib = Arc::new(Library::new(fake_edge(state.clone()), noop_warm(), now));

        let servers = lib.featured().await;
        // Every featured publisher resolves; github.com arrives via the
        // documented map (registry miss + well-known 404), the rest via registry.
        assert_eq!(servers.len(), FEATURED_DOMAINS.len());
        assert!(servers.iter().all(|s| s.tier == Tier::FirstParty));
        assert_eq!(
            servers
                .iter()
                .find(|s| s.domain.as_deref() == Some("github.com"))
                .unwrap()
                .url,
            "https://api.githubcopilot.com/mcp/"
        );
        // The whole point of the pool: 32 publishers resolved, never more
        // than 6 registry requests in flight — and exactly 6, so the pool ran
        // full rather than collapsing to sequential.
        let s = state.lock().unwrap();
        assert_eq!(s.registry_max_in_flight, 6);
        let searches = s
            .calls
            .iter()
            .filter(|c| c.starts_with("registry.modelcontextprotocol.io"))
            .count();
        assert_eq!(searches, FEATURED_DOMAINS.len());
        assert!(
            s.calls
                .iter()
                .any(|c| c == "https://github.com/.well-known/mcp.json")
        );
    }

    #[tokio::test]
    async fn serves_a_fresh_featured_shelf_with_zero_upstream_calls() {
        let registry = registry_with(FEATURED_DOMAINS, &["github.com"], "OLD");
        let state = state_with(registry);
        let (_t, now) = clock();
        let lib = Arc::new(Library::new(fake_edge(state.clone()), noop_warm(), now));

        lib.featured().await;
        state.lock().unwrap().calls.clear();
        let again = lib.featured().await;
        assert_eq!(again.len(), FEATURED_DOMAINS.len());
        assert!(state.lock().unwrap().calls.is_empty());
    }

    #[tokio::test]
    async fn serves_a_stale_shelf_immediately_and_revalidates_single_flighted() {
        let registry = registry_with(FEATURED_DOMAINS, &["github.com"], "OLD");
        let state = state_with(registry);
        let (t, now) = clock();
        let lib = Arc::new(Library::new(fake_edge(state.clone()), noop_warm(), now));

        let first = lib.featured().await;
        assert_eq!(
            first
                .iter()
                .find(|s| s.domain.as_deref() == Some("stripe.com"))
                .unwrap()
                .description
                .as_deref(),
            Some("OLD stripe.com")
        );

        // Age past the 1h shelf TTL, change the registry's answer (only
        // stripe and linear still resolve), and gate every upstream fetch so
        // NOTHING can complete until we release it.
        *t.lock().unwrap_or_else(|p| p.into_inner()) += 61 * 60 * 1000;
        {
            let mut s = state.lock().unwrap();
            let misses: Vec<&str> = FEATURED_DOMAINS
                .iter()
                .filter(|d| **d != "stripe.com" && **d != "linear.app")
                .copied()
                .collect();
            s.registry = registry_with(FEATURED_DOMAINS, &misses, "NEW");
            s.gate = Some(Vec::new());
        }
        let before_refresh = state.lock().unwrap().calls.len();

        // Two concurrent STALE serves: both answer now, from cache, with the
        // old data — with the network gated and nobody releasing it, the
        // join! completing at all proves neither caller awaited the network.
        let (a, b) = futures_util::join!(lib.featured(), lib.featured());
        assert_eq!(a, b);
        assert_eq!(
            a.iter()
                .find(|s| s.domain.as_deref() == Some("stripe.com"))
                .unwrap()
                .description
                .as_deref(),
            Some("OLD stripe.com")
        );

        // Release the gates and let the one background fan-out finish. Poll
        // on the OUTCOME (the shelf being replaced), not on fetch counts —
        // the last fetch STARTING is not the shelf BEING REPLACED. The count
        // is asserted after: 32 registry searches + 30 well-known probes, no
        // more — single-flight is what pins it, since two concurrent stale
        // serves without it would double the fan-out. (Snapshot polling, not
        // featured() polling: with the clock frozen the re-landed shelf and a
        // re-request share the same millisecond, so the public path would
        // spawn a fresh refresh per poll iteration.)
        {
            let mut s = state.lock().unwrap();
            if let Some(gate) = s.gate.take() {
                for release in gate {
                    let _ = release.send(());
                }
            }
        }
        let mut landed = None;
        for _ in 0..100_000 {
            if let Some((_, shelf)) = lib.featured_snapshot()
                && shelf.iter().any(|s| {
                    s.domain.as_deref() == Some("stripe.com")
                        && s.description.as_deref() == Some("NEW stripe.com")
                })
            {
                landed = Some(shelf);
                break;
            }
            tokio::task::yield_now().await;
        }
        let shelf = landed.expect("the background revalidation never landed");
        assert_eq!(shelf.len(), 3); // stripe + linear from the registry, github documented
        let refresh_calls = FEATURED_DOMAINS.len() // every publisher re-resolved past the 1h TTL
            + FEATURED_DOMAINS
                .iter()
                .filter(|d| **d != "stripe.com" && **d != "linear.app")
                .count(); // each miss probed /.well-known/mcp.json (404)
        assert_eq!(
            state.lock().unwrap().calls.len(),
            before_refresh + refresh_calls
        );
    }

    #[tokio::test]
    async fn pins_a_brand_search_from_the_warmed_publisher_cache() {
        let registry = registry_with(FEATURED_DOMAINS, &["github.com"], "OLD");
        let state = state_with(registry);
        let (_t, now) = clock();
        let lib = Arc::new(Library::new(fake_edge(state.clone()), noop_warm(), now));
        lib.featured().await; // warms publisher_cache for every featured domain
        state.lock().unwrap().calls.clear();

        let results = lib.search("stripe").await;
        // One registry page for the search itself; the stripe.com publisher
        // pin comes from cache (a live resolve would add a search + probe).
        assert_eq!(state.lock().unwrap().calls.len(), 1);
        assert_eq!(results[0].domain.as_deref(), Some("stripe.com"));
        assert_eq!(results[0].tier, Tier::FirstParty);
    }

    #[tokio::test]
    async fn the_job_spec_carries_the_declared_timings() {
        let registry = registry_with(&[], &[], "OLD");
        let state = state_with(registry);
        let (_t, now) = clock();
        let lib = Arc::new(Library::new(fake_edge(state), noop_warm(), now));
        let spec = mcp_library_refresh_job_spec(lib);
        assert_eq!(spec.name.as_str(), "mcp-library-refresh");
        assert_eq!(spec.every_ms, 30 * 60_000);
        assert_eq!(spec.first_run_delay_ms, Some(10_000));
        assert_eq!(spec.max_run_ms, Some(2 * 60_000));
        assert!(
            spec.per_instance,
            "the cache being warmed is process memory"
        );
    }
}
