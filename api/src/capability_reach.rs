// CAN THIS DEPLOYMENT DO IT — as opposed to: can this model do it.
// Port of capability-reach.ts, with capability-platform.ts's supply half (the
// two files cross together because each is the other's reason to exist).
//
// THE CATEGORY ERROR THIS FIXES, in the words of the bug report that found it:
// "search can be TOOL DRIVEN in Talaria, so saying a MODEL is not capable is
// true, but at the harness level models are slotting into, it's completely
// untrue."
//
// That is exactly right. `capability.rs` answers "does the MODEL do this
// natively" — an attribute of weights and of the endpoint serving them. THIS
// module answers "can the RUN reach it": natively, or through a tool the
// platform will supply. That is the question a floor should ask before
// refusing, because it is the question the admin is actually asking when they
// pick a model from a dropdown.
//
// WHAT IT DOES NOT DO: invent reach. A tool path counts only when the tool is
// REALLY THERE — registered and enabled in this install, or supplied by
// Talaria itself and CHECKED to be working — and the model can actually call
// tools. An org with neither gets the same "not a fit" it got before, with a
// materially better sentence naming the thing to go and install rather than
// blaming the model.

use crate::capability::{CapabilityFact, get_capabilities};
use crate::gateway::settings::get_setting;
use crate::model_roles::resolve_role_model;
use futures_util::future::join_all;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How a capability is satisfied for one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReachVia {
    Native,
    Tool,
}

/// The registered tool that supplies a capability (`via == Tool`). Serializable
/// because it rides in a run's CHECKPOINT: a research run resolved to the tool
/// path must keep driving through the same supplier across re-entries, and the
/// checkpoint column is where that resolution survives (serde shape = the TS
/// `{ server, tool }` exactly, so TS-written rows re-enter here and back).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Supplier {
    pub server: String,
    pub tool: String,
}

/// How one capability stands for one run. `detail` is always populated — the
/// negative case is the one that has to say what to do about it, written for
/// the model picker rather than for a developer.
#[derive(Clone)]
pub struct Reach {
    pub capability: String,
    pub reached: bool,
    pub via: Option<ReachVia>,
    pub supplier: Option<Supplier>,
    pub detail: String,
}

// ── Which capabilities a tool can stand in for ───────────────────────────────

/// THE TABLE IS SHORT ON PURPOSE: an entry earns its place by being a case
/// where a tool genuinely does the job, not by being conceivable. `search`
/// qualifies completely (a web-search tool returns the same passages-with-URLs
/// a sonar model returns, and the synthesis stage cannot tell which produced
/// them). `vision` qualifies through `describe_image`, which reads the image
/// with the model the org assigned to the vision role.
///
/// DELIBERATELY ABSENT, so the next author does not have to relitigate:
/// `code` (the workbench RUNS code; the capability is about WRITING it),
/// `long-context` (chunking is a strategy, not a capability), and the
/// decoding-shape capabilities (`json`, `json-strict`, `tools`,
/// `tool-select`, `instruction-following`) — properties of the model's own
/// decoding; there is no outside thing to hand it.
struct ToolReachRule {
    capability: &'static str,
    /// Tool names that supply it, lowercased. Matched whole-word so
    /// `search_knowledge` (Talaria's own RAG over the org's docs, not the
    /// live web) cannot be mistaken for a web-search tool.
    names: &'static [&'static str],
    /// Words in a tool's DESCRIPTION that corroborate a name match.
    hints: &'static [&'static str],
}

const TOOL_REACHABLE: &[ToolReachRule] = &[
    ToolReachRule {
        capability: "search",
        names: &[
            "web_search",
            "websearch",
            "search_web",
            "brave_web_search",
            "tavily_search",
            "exa_search",
            "google_search",
            "perplexity_search",
            "serper_search",
        ],
        hints: &["web", "internet", "live", "browse", "online"],
    },
    ToolReachRule {
        capability: "vision",
        names: &[
            "describe_image",
            "read_image",
            "analyze_image",
            "image_describe",
            "vision",
            "ocr",
        ],
        hints: &[
            "image",
            "screenshot",
            "photo",
            "picture",
            "visual",
            "chart",
            "scan",
        ],
    },
];

/// An ADMIN'S OWN WIRING, which always wins over the heuristic below. The map
/// value is the TS `CapabilityProviders` entry exactly: absent key = nothing
/// said; `None` = an explicit "nothing supplies this here"; `Some(pin)` = the
/// supplier, however odd its name. (A malformed row — neither null nor an
/// object — is dropped rather than guessed at; the write path validates, so
/// the shapes here are the ones the admin UI writes.)
pub type Providers = HashMap<String, Option<Supplier>>;

/// The setting key the admin's wiring lives under.
pub const PROVIDERS_KEY: &str = "capability_providers";

/// The two columns of an MCP registry row this module owns. The full registry
/// type crosses with the MCP plane; reach reads names, enabledness and tools,
/// and deliberately nothing else.
#[derive(Clone)]
pub struct ReachServer {
    pub name: String,
    pub enabled: bool,
    pub tools: Vec<ReachTool>,
}

#[derive(Clone)]
pub struct ReachTool {
    pub name: String,
    pub description: Option<String>,
}

/// A TOOL TALARIA ITSELF SUPPLIES, which is in nobody's MCP registry. A LAST
/// RESORT, NOT A DEFAULT — the registry is consulted first, because an org
/// that installed Exa or Tavily chose it and an admin pin beats both — AND
/// CHECKED, NOT ASSUMED: every entry is produced by a caller that has
/// confirmed the thing actually works here (SearXNG answering the canary, a
/// model assigned to the vision role). Claiming reach through a tool that
/// 503s would convert a red cell an admin can act on into a green one they
/// cannot.
#[derive(Clone)]
pub struct PlatformSupply {
    pub capability: &'static str,
    pub server: &'static str,
    pub tool: &'static str,
}

/// THE SERVER NAME TALARIA USES FOR ITS OWN TOOLS, shown to admins beside a
/// `supplied` tag so it reads as a place rather than an implementation detail.
pub const PLATFORM_SERVER: &str = "talaria";

/// The half of `ReachDeps` a test drives; the real half is `DbReach`.
/// Async-in-trait is deliberate: this trait is crate-internal (the runner
/// wraps the real impl in a boxed closure), and the returned futures are Send
/// because every await inside is sqlx or reqwest.
#[allow(async_fn_in_trait)]
pub trait ReachDeps {
    async fn servers(&self) -> Vec<ReachServer>;
    async fn providers(&self) -> Providers;
    async fn capabilities(&self, key: &str) -> HashMap<String, CapabilityFact>;
    async fn platform(&self) -> Vec<PlatformSupply>;
}

/// Whole-word match, so `search_knowledge` does not answer for `search`.
fn name_matches(tool_name: &str, names: &[&str]) -> bool {
    let n = tool_name.to_lowercase();
    names.iter().any(|want| {
        n == *want || n.ends_with(&format!("_{want}")) || n.starts_with(&format!("{want}_"))
    })
}

fn platform_for(capability: &str, platform: &[PlatformSupply]) -> Option<Supplier> {
    platform
        .iter()
        .find(|p| p.capability == capability)
        .map(|p| Supplier {
            server: p.server.to_string(),
            tool: p.tool.to_string(),
        })
}

/// The registered, ENABLED tool that supplies this capability, or None. Pure
/// over the server list so the whole rule is tested without a database and
/// without an MCP server anywhere near it.
pub fn supplier_for(
    capability: &str,
    servers: &[ReachServer],
    providers: &Providers,
    platform: &[PlatformSupply],
) -> Option<Supplier> {
    match providers.get(capability) {
        // An explicit None is an admin saying "nothing supplies this here" —
        // a deliberate answer, and not the same as having said nothing. It
        // silences the platform's own tool too.
        Some(None) => return None,
        Some(pin) => {
            let pinned = pin.as_ref().unwrap();
            if let Some(srv) = servers
                .iter()
                .find(|s| s.name == pinned.server && s.enabled)
                && srv.tools.iter().any(|t| t.name == pinned.tool)
            {
                return Some(pinned.clone());
            }
            // A pin whose server has gone away falls through to the
            // platform's own tool rather than to nothing: the admin's answer
            // to "which supplier" is stale, but their answer to "should this
            // be supplied" was yes.
            return platform_for(capability, platform);
        }
        None => {}
    }

    let rule = TOOL_REACHABLE.iter().find(|r| r.capability == capability)?;
    for srv in servers.iter().filter(|s| s.enabled) {
        for tool in &srv.tools {
            if !name_matches(&tool.name, rule.names) {
                continue;
            }
            // A name match alone is enough when the tool publishes no
            // description; when it does, one corroborating word keeps a
            // same-named intranet search from being read as the live web.
            let desc = tool.description.as_deref().unwrap_or("").to_lowercase();
            if !desc.is_empty() && !rule.hints.iter().any(|h| desc.contains(h)) {
                continue;
            }
            return Some(Supplier {
                server: srv.name.clone(),
                tool: tool.name.clone(),
            });
        }
    }
    // Nothing registered offers it. Talaria's own surface is the floor.
    platform_for(capability, platform)
}

/// CAN THIS RUN REACH THESE CAPABILITIES, and how. `keys` are the capability
/// keys the model resolves to — the same endpoint:model keys the runner
/// derives, passed in rather than re-derived so this file never becomes a
/// second spelling of that rule. A capability counts as native only when
/// EVERY key says so, the same unanimity `missing_capabilities` applies: a
/// bare id can land on any endpoint in the pool, and a claim has to hold for
/// the worst of them.
pub async fn reach_for<D: ReachDeps + Sync>(
    d: &D,
    keys: &[String],
    wanted: &[&str],
) -> HashMap<String, Reach> {
    let mut out = HashMap::new();
    if wanted.is_empty() {
        return out;
    }

    let facts: Vec<HashMap<String, CapabilityFact>> =
        join_all(keys.iter().map(|k| d.capabilities(k))).await;
    let native_yes =
        |cap: &str| !keys.is_empty() && facts.iter().all(|f| f.get(cap).is_some_and(|c| c.value));
    let native_no =
        |cap: &str| !keys.is_empty() && facts.iter().all(|f| f.get(cap).is_some_and(|c| !c.value));

    // Only read the registry if something might need a tool. An install with
    // no tool-reachable requirement should not pay for the query.
    let needs_tools = wanted
        .iter()
        .any(|c| TOOL_REACHABLE.iter().any(|r| r.capability == *c));
    let (servers, providers, platform) = if needs_tools {
        (d.servers().await, d.providers().await, d.platform().await)
    } else {
        (Vec::new(), Providers::new(), Vec::new())
    };

    for cap in wanted {
        let supplier = supplier_for(cap, &servers, &providers, &platform);
        // TOOL-FIRST, FOR `search` ONLY. Nearly every model that "has web
        // search" only searches WHEN ASKED — a provider plugin, an `:online`
        // suffix — so for a model nobody ASKS to search, "native" means: it
        // answers from memory and the run ends with no sources. The tool path
        // works for every model, so it is the default; an org that would
        // rather spend a sonar model's own index pins
        // `capability_providers.search` to null, and native is all that is
        // left. `vision`'s tool stand-in is genuinely lossier than a model
        // that reads the image itself, so it is NOT demoted the same way.
        let tool_first = *cap == "search" && supplier.is_some() && !native_no("tools");
        if native_yes(cap) && !tool_first {
            out.insert(
                (*cap).to_string(),
                Reach {
                    capability: cap.to_string(),
                    reached: true,
                    via: Some(ReachVia::Native),
                    supplier: None,
                    detail: format!("the model does '{cap}' itself"),
                },
            );
            continue;
        }

        if let Some(supplier) = supplier {
            // THE MODEL STILL HAS TO BE ABLE TO CALL THE TOOL. A search
            // server in front of a model that cannot hold a tool call is not
            // reach — it is a model that will answer from memory with a tool
            // sitting unused beside it, which is the exact failure the search
            // floor exists to prevent.
            if native_no("tools") {
                out.insert(
                    (*cap).to_string(),
                    Reach {
                        capability: cap.to_string(),
                        reached: false,
                        via: None,
                        supplier: None,
                        detail: format!(
                            "'{}.{}' could supply '{}', but this model is recorded as unable to call tools.",
                            supplier.server, supplier.tool, cap
                        ),
                    },
                );
                continue;
            }
            out.insert(
                (*cap).to_string(),
                Reach {
                    capability: cap.to_string(),
                    reached: true,
                    via: Some(ReachVia::Tool),
                    supplier: Some(supplier.clone()),
                    detail: format!(
                        "the model calls '{}.{}' for it",
                        supplier.server, supplier.tool
                    ),
                },
            );
            continue;
        }

        // NOT REACHED, and the sentence has to say which of the two reasons —
        // the model cannot, or the org has not installed the thing that could.
        let reachable = TOOL_REACHABLE.iter().any(|r| r.capability == *cap);
        let detail = if reachable {
            format!(
                "nothing here reaches '{cap}': the model does not do it natively and no enabled MCP server offers a tool for it. Register one, or assign a model that does '{cap}' itself."
            )
        } else if native_no(cap) {
            format!(
                "the model is recorded as not supporting '{cap}', and nothing can supply it on the model's behalf."
            )
        } else {
            format!("nothing has measured '{cap}' on this model.")
        };
        out.insert(
            (*cap).to_string(),
            Reach {
                capability: cap.to_string(),
                reached: false,
                via: None,
                supplier: None,
                detail,
            },
        );
    }
    out
}

// ── Talaria's own supply, checked (capability-platform.ts) ──────────────────

/// How long a probe of our own tool may take before we call it unavailable.
/// Short: this runs on the path that opens the fitness page and starts a
/// sweep, and a supplier that takes five seconds to answer "yes" is one the
/// harness's own turn budget would have failed on anyway.
const CHECK_MS: Duration = Duration::from_millis(4_000);

// The canary word itself ("wikipedia" — a common term, so a working small
// index cannot report itself down) is search.rs's own constant now, with the
// reasoning beside it. So is where SearXNG lives (search.rs `search_url`):
// the reach probe and a live search can never disagree about where the engine
// is, because there is one spelling.

/// `searchReachable` reduced to the question platform supply asks: is the
/// org's web-search backend answering with results? ONE canary query through
/// the real client (search.rs) — bounded here by CHECK_MS so a dead instance
/// cannot stall the reach edge. This probe USED to be a second spelling of the
/// GET; it rides the full client now, which is what its own note said would
/// happen when the client crossed.
async fn search_canary_ok(pg: &PgPool) -> bool {
    match tokio::time::timeout(
        CHECK_MS,
        crate::search::search_reachable(pg, &crate::search::real_deps()),
    )
    .await
    {
        Ok(reply) => reply.ok,
        Err(_) => false,
    }
}

/// HOW LONG AN ANSWER IS GOOD FOR. This is not an optimisation, it is a
/// correctness fix: `reach_for` sits on the research hot path, and an
/// uncached platform supply would put a live HTTP probe of SearXNG in front
/// of every run — a four-second one, in front of every run where SearXNG was
/// down. A minute, because these answers change on the timescale of an admin
/// editing a setting or a container restarting, and a stale "we can search"
/// costs one harness run that fails honestly.
const CACHE_MS: Duration = Duration::from_secs(60);

/// The cached answer, with its stamp.
type SupplyCache = Mutex<Option<(Instant, Vec<PlatformSupply>)>>;

fn supply_cache() -> &'static SupplyCache {
    static CELL: OnceLock<SupplyCache> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// Forget the cached answer — for tests, and for the settings route that
/// changes the search URL (an admin who has just fixed it should not wait
/// out a minute of the old answer).
pub fn forget_platform_supply() {
    if let Ok(mut cell) = supply_cache().lock() {
        *cell = None;
    }
}

/// Everything this deployment can stand in for, CHECKED. Empty is a perfectly
/// ordinary answer — an install with no search backend and no vision role
/// supplies nothing, and should say so rather than pretend. Never fails: a
/// probe that cannot answer is a probe that answered no.
pub async fn platform_supply(pg: &PgPool) -> Vec<PlatformSupply> {
    if let Ok(cell) = supply_cache().lock()
        && let Some((at, value)) = cell.as_ref()
        && at.elapsed() < CACHE_MS
    {
        return value.clone();
    }
    // `web_search` is the one tool on Talaria's native surface, and
    // `research-search` is the harness that offers it.
    let search_ok = search_canary_ok(pg).await;
    // `describe_image` reads an image with the ROLE model, so the role being
    // filled is exactly the condition — the vision floor refuses rather than
    // degrades when it is not.
    let vision = resolve_role_model(pg, "vision")
        .await
        .ok()
        .flatten()
        .is_some();
    let mut out = Vec::new();
    if search_ok {
        out.push(PlatformSupply {
            capability: "search",
            server: PLATFORM_SERVER,
            tool: "web_search",
        });
    }
    if vision {
        out.push(PlatformSupply {
            capability: "vision",
            server: PLATFORM_SERVER,
            tool: "describe_image",
        });
    }
    if let Ok(mut cell) = supply_cache().lock() {
        *cell = Some((Instant::now(), out.clone()));
    }
    out
}

// ── The real deps ────────────────────────────────────────────────────────────

/// The reach edge over the live install. Every read degrades to "nothing"
/// rather than erroring — a registry that is down must not become a run that
/// claims reach it cannot honor, and must not fail a turn that would have
/// worked without the question.
pub struct DbReach<'a> {
    pub pg: &'a PgPool,
}

impl ReachDeps for DbReach<'_> {
    async fn servers(&self) -> Vec<ReachServer> {
        let Ok(rows) = sqlx::query_as::<_, (String, bool, serde_json::Value)>(
            "select name, enabled, tools from mcp_servers order by builtin desc, name",
        )
        .fetch_all(self.pg)
        .await
        else {
            return Vec::new();
        };
        rows.into_iter()
            .map(|(name, enabled, tools)| ReachServer {
                name,
                enabled,
                tools: tools
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|t| {
                                Some(ReachTool {
                                    name: t.get("name")?.as_str()?.to_string(),
                                    description: t
                                        .get("description")
                                        .and_then(|d| d.as_str())
                                        .map(String::from),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect()
    }

    async fn providers(&self) -> Providers {
        let raw = get_setting(self.pg, PROVIDERS_KEY, serde_json::json!({})).await;
        let mut out = Providers::new();
        let Some(obj) = raw.as_object() else {
            return out;
        };
        for (cap, v) in obj {
            if v.is_null() {
                out.insert(cap.clone(), None);
            } else if let (Some(server), Some(tool)) = (
                v.get("server").and_then(|s| s.as_str()),
                v.get("tool").and_then(|t| t.as_str()),
            ) {
                out.insert(
                    cap.clone(),
                    Some(Supplier {
                        server: server.to_string(),
                        tool: tool.to_string(),
                    }),
                );
            }
        }
        out
    }

    async fn capabilities(&self, key: &str) -> HashMap<String, CapabilityFact> {
        get_capabilities(self.pg, key).await
    }

    async fn platform(&self) -> Vec<PlatformSupply> {
        platform_supply(self.pg).await
    }
}

/// The one-call reach edge the runner injects: keys are the capability keys
/// the model resolved to, `wanted` the capabilities a floor is asking about.
pub async fn reach_for_keys(
    pg: &PgPool,
    keys: &[String],
    wanted: &[&str],
) -> HashMap<String, Reach> {
    reach_for(&DbReach { pg }, keys, wanted).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // THE QUESTION THIS FILE PINS DOWN is the one the fitness page used to
    // get wrong: not "can the model do it" but "can the RUN do it". A model
    // measured at 100% tool calling, with a web-search server registered,
    // reaches search — and reporting it Not-a-fit for Research on the
    // strength of `search: false` cost an admin a capable model for no reason.
    struct Fake {
        servers: Vec<ReachServer>,
        providers: Providers,
        facts: HashMap<String, HashMap<String, CapabilityFact>>,
        platform: Vec<PlatformSupply>,
        asked: std::sync::atomic::AtomicUsize,
    }

    impl Fake {
        fn new() -> Self {
            Fake {
                servers: Vec::new(),
                providers: Providers::new(),
                facts: HashMap::new(),
                platform: Vec::new(),
                asked: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn fact(&mut self, key: &str, cap: &str, value: bool) {
            self.facts.entry(key.to_string()).or_default().insert(
                cap.to_string(),
                CapabilityFact {
                    value,
                    source: "probe".into(),
                    at: "2026-08-07T00:00:00.000Z".into(),
                    detail: None,
                    score: None,
                },
            );
        }
    }

    impl ReachDeps for Fake {
        async fn servers(&self) -> Vec<ReachServer> {
            self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.servers.clone()
        }
        async fn providers(&self) -> Providers {
            self.providers.clone()
        }
        async fn capabilities(&self, key: &str) -> HashMap<String, CapabilityFact> {
            self.facts.get(key).cloned().unwrap_or_default()
        }
        async fn platform(&self) -> Vec<PlatformSupply> {
            self.platform.clone()
        }
    }

    fn server(name: &str, enabled: bool, tools: &[(&str, Option<&str>)]) -> ReachServer {
        ReachServer {
            name: name.into(),
            enabled,
            tools: tools
                .iter()
                .map(|(n, d)| ReachTool {
                    name: (*n).into(),
                    description: d.map(String::from),
                })
                .collect(),
        }
    }

    fn search_server(enabled: bool) -> ReachServer {
        server(
            "exa",
            enabled,
            &[(
                "web_search",
                Some("Search the live web for current information."),
            )],
        )
    }

    const KEY: &str = "openrouter:deepseek/deepseek-v4-flash";

    fn sup(s: &str, t: &str) -> Supplier {
        Supplier {
            server: s.into(),
            tool: t.into(),
        }
    }

    #[test]
    fn supplier_by_name_corroborated_by_description() {
        let providers = Providers::new();
        assert_eq!(
            supplier_for("search", &[search_server(true)], &providers, &[]),
            Some(sup("exa", "web_search"))
        );
    }

    #[test]
    fn the_orgs_own_rag_tool_is_not_the_live_web() {
        // `search_knowledge` searches the org's indexed documents. A substring
        // match on "search" would read it as the live web and let a research
        // run answer from the company wiki while claiming to have browsed.
        let rag = server(
            "talaria",
            true,
            &[("search_knowledge", Some("Search the org knowledge base."))],
        );
        assert_eq!(supplier_for("search", &[rag], &Providers::new(), &[]), None);
    }

    #[test]
    fn a_disabled_server_supplies_nothing() {
        assert_eq!(
            supplier_for("search", &[search_server(false)], &Providers::new(), &[]),
            None
        );
    }

    #[test]
    fn a_self_describing_tool_needs_one_corroborating_word() {
        // A same-named tool that searches an intranet is a false positive,
        // and the description is the only thing that can tell them apart.
        let intranet = server(
            "exa",
            true,
            &[("web_search", Some("Search the internal SharePoint index."))],
        );
        assert_eq!(
            supplier_for("search", &[intranet], &Providers::new(), &[]),
            None
        );
        // With no description at all, the name is all there is and it is
        // enough.
        let bare = server("exa", true, &[("web_search", None)]);
        assert_eq!(
            supplier_for("search", &[bare], &Providers::new(), &[]),
            Some(sup("exa", "web_search"))
        );
    }

    #[test]
    fn an_admin_pin_beats_the_heuristic() {
        let odd = server("house", true, &[("q", None)]);
        let mut providers = Providers::new();
        providers.insert("search".into(), Some(sup("house", "q")));
        assert_eq!(
            supplier_for("search", &[odd], &providers, &[]),
            Some(sup("house", "q"))
        );
    }

    #[test]
    fn an_admin_null_overrides_a_match() {
        // An explicit null is a deliberate answer and not the same as silence
        // — it silences the platform's own tool too.
        let mut providers = Providers::new();
        providers.insert("search".into(), None);
        assert_eq!(
            supplier_for("search", &[search_server(true)], &providers, &[]),
            None
        );
        assert_eq!(
            supplier_for(
                "search",
                &[],
                &providers,
                &[PlatformSupply {
                    capability: "search",
                    server: PLATFORM_SERVER,
                    tool: "web_search"
                }]
            ),
            None
        );
    }

    #[test]
    fn a_stale_pin_falls_through_to_the_platform_not_nothing() {
        // The admin's answer to "which supplier" is stale, but their answer
        // to "should this be supplied" was yes.
        let mut providers = Providers::new();
        providers.insert("search".into(), Some(sup("gone", "web_search")));
        let platform = [PlatformSupply {
            capability: "search",
            server: PLATFORM_SERVER,
            tool: "web_search",
        }];
        assert_eq!(
            supplier_for("search", &[], &providers, &platform),
            Some(sup(PLATFORM_SERVER, "web_search"))
        );
        // A pin onto a server that exists but lacks the tool is stale too.
        let empty = server("exa", true, &[]);
        let mut providers = Providers::new();
        providers.insert("search".into(), Some(sup("exa", "web_search")));
        assert_eq!(
            supplier_for(
                "search",
                std::slice::from_ref(&empty),
                &providers,
                &platform
            ),
            Some(sup(PLATFORM_SERVER, "web_search"))
        );
        // And with no platform tool under it, nothing at all.
        assert_eq!(supplier_for("search", &[empty], &providers, &[]), None);
    }

    #[test]
    fn no_tool_stands_in_for_code_long_context_or_json() {
        // A sandbox is not a programmer, and a tool cannot widen a context
        // window. VISION IS NO LONGER ON THIS LIST — `describe_image` reads
        // an image with the model the org assigned to the vision role.
        let srv = search_server(true);
        for cap in ["code", "long-context", "json"] {
            assert_eq!(
                supplier_for(cap, std::slice::from_ref(&srv), &Providers::new(), &[]),
                None
            );
        }
    }

    #[test]
    fn the_image_reader_supplies_vision() {
        let talaria = server(
            "talaria",
            true,
            &[(
                "describe_image",
                Some(
                    "Read an image you cannot see: attaches it to this workspace's vision model and returns a description in text.",
                ),
            )],
        );
        assert_eq!(
            supplier_for("vision", &[talaria], &Providers::new(), &[]),
            Some(sup("talaria", "describe_image"))
        );
    }

    #[test]
    fn vision_needs_description_corroboration_like_search_does() {
        // `vision` is an ordinary English word and `ocr` is a narrow one, so
        // the name alone is weaker evidence here than it is for `web_search`.
        let reader = server(
            "house",
            true,
            &[("ocr", Some("Extract text from a scanned image or photo."))],
        );
        assert_eq!(
            supplier_for("vision", &[reader], &Providers::new(), &[]),
            Some(sup("house", "ocr"))
        );
        let unrelated = server(
            "house",
            true,
            &[("vision", Some("Company vision and mission statements."))],
        );
        assert_eq!(
            supplier_for("vision", &[unrelated], &Providers::new(), &[]),
            None
        );
    }

    #[tokio::test]
    async fn native_reach_when_the_model_browses() {
        let mut d = Fake::new();
        d.fact(KEY, "search", true);
        let out = reach_for(&d, &[KEY.into()], &["search"]).await;
        let r = out.get("search").unwrap();
        assert!(r.reached);
        assert_eq!(r.via, Some(ReachVia::Native));
    }

    #[tokio::test]
    async fn tool_reach_when_the_model_cannot_browse_but_can_call() {
        // THE CASE THE WHOLE CHANGE IS FOR. deepseek-v4-flash: search false,
        // tools true, and a web-search server in the registry. It can do
        // research.
        let mut d = Fake::new();
        d.servers = vec![search_server(true)];
        d.fact(KEY, "search", false);
        d.fact(KEY, "tools", true);
        let out = reach_for(&d, &[KEY.into()], &["search"]).await;
        let r = out.get("search").unwrap();
        assert!(r.reached);
        assert_eq!(r.via, Some(ReachVia::Tool));
        assert_eq!(r.supplier, Some(sup("exa", "web_search")));
    }

    #[tokio::test]
    async fn a_real_search_tool_beats_a_native_search_claim() {
        // The bug this ordering fixes: a model recorded `search: true` took
        // the native path — a plain completion, no plugin — and answered from
        // memory. Nothing here ever asks a model to browse.
        let mut d = Fake::new();
        d.servers = vec![search_server(true)];
        d.fact(KEY, "search", true);
        d.fact(KEY, "tools", true);
        let out = reach_for(&d, &[KEY.into()], &["search"]).await;
        let r = out.get("search").unwrap();
        assert_eq!(r.via, Some(ReachVia::Tool));
        assert_eq!(r.supplier, Some(sup("exa", "web_search")));
    }

    #[tokio::test]
    async fn an_admin_null_leaves_a_browsing_model_native() {
        // The escape hatch for a sonar model, whose own index beats looping
        // it through a web-search tool.
        let mut d = Fake::new();
        d.servers = vec![search_server(true)];
        d.providers.insert("search".into(), None);
        d.fact(KEY, "search", true);
        d.fact(KEY, "tools", true);
        let out = reach_for(&d, &[KEY.into()], &["search"]).await;
        assert_eq!(out.get("search").unwrap().via, Some(ReachVia::Native));
    }

    #[tokio::test]
    async fn a_model_that_can_see_stays_native() {
        // Only `search` prefers its tool. Captioning is lossier than a model
        // reading the image itself, so vision must not be demoted.
        let mut d = Fake::new();
        d.servers = vec![server(
            "talaria",
            true,
            &[("describe_image", Some("Describe an image."))],
        )];
        d.fact(KEY, "vision", true);
        d.fact(KEY, "tools", true);
        let out = reach_for(&d, &[KEY.into()], &["vision"]).await;
        assert_eq!(out.get("vision").unwrap().via, Some(ReachVia::Native));
    }

    #[tokio::test]
    async fn unknown_is_not_false() {
        let mut d = Fake::new();
        d.servers = vec![search_server(true)];
        let out = reach_for(&d, &[KEY.into()], &["search"]).await;
        assert_eq!(out.get("search").unwrap().via, Some(ReachVia::Tool));
    }

    #[tokio::test]
    async fn no_tool_reach_when_the_model_cannot_call_tools() {
        // A search server in front of a model that cannot hold a tool call
        // is not reach — it is a model answering from memory with a tool
        // sitting unused beside it.
        let mut d = Fake::new();
        d.servers = vec![search_server(true)];
        d.fact(KEY, "search", false);
        d.fact(KEY, "tools", false);
        let out = reach_for(&d, &[KEY.into()], &["search"]).await;
        let r = out.get("search").unwrap();
        assert!(!r.reached);
        assert!(r.detail.contains("unable to call tools"));
    }

    #[tokio::test]
    async fn no_reach_names_the_thing_to_install() {
        let mut d = Fake::new();
        d.fact(KEY, "search", false);
        let out = reach_for(&d, &[KEY.into()], &["search"]).await;
        let r = out.get("search").unwrap();
        assert!(!r.reached);
        // The sentence has to name the org's next move, not blame the model.
        assert!(r.detail.contains("Register one"));
    }

    #[tokio::test]
    async fn native_needs_unanimity_across_the_pool() {
        // A bare id can land on any member, so a native claim has to hold
        // for the worst of them; the tool path is what rescues the mixed
        // case — and with no server registered, the mixed case does not
        // reach at all.
        let mut d = Fake::new();
        d.fact("a", "search", true);
        d.fact("b", "search", false);
        let out = reach_for(&d, &["a".into(), "b".into()], &["search"]).await;
        assert!(!out.get("search").unwrap().reached);
    }

    #[tokio::test]
    async fn the_registry_is_not_read_for_an_unreachable_capability() {
        // `code` HAS NO SUPPLIER RULE and cannot get one: writing code is
        // what the model does, not something a tool can do on its behalf.
        let mut d = Fake::new();
        d.fact(KEY, "code", false);
        let out = reach_for(&d, &[KEY.into()], &["code"]).await;
        assert_eq!(d.asked.load(std::sync::atomic::Ordering::SeqCst), 0);
        let r = out.get("code").unwrap();
        assert!(!r.reached);
        assert_eq!(r.via, None);
        assert!(r.detail.contains("nothing can supply it"));
    }

    #[tokio::test]
    async fn asking_for_nothing_reads_nothing() {
        let d = Fake::new();
        assert!(reach_for(&d, &[KEY.into()], &[]).await.is_empty());
    }

    #[test]
    fn whole_word_matching() {
        assert!(name_matches("web_search", &["web_search"]));
        assert!(name_matches("exa_web_search", &["web_search"]));
        assert!(name_matches("web_search_pro", &["web_search"]));
        assert!(!name_matches("search_knowledge", &["web_search"]));
        assert!(!name_matches("websearching", &["web_search"]));
    }
}
