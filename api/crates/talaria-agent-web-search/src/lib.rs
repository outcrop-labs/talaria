// AGENT WEB SEARCH — the operator's one choice of web provider for every
// fleet agent, rendered into each agent's own Hermes.
//
// WHY THIS IS A SETTING AND NOT A TOOL. Hermes carries its own web provider
// registry (`plugins/web/<vendor>/` in v0.22.4). Its `web_search` and
// `web_extract` tools pick a backend from `web.search_backend` /
// `web.extract_backend` in config.yaml and read each vendor's key from the
// process environment. So "agents search with the provider the operator
// picked" is a few config keys plus one env var per agent: rendered here,
// spent there. Talaria writes no vendor client for agents. Research is a
// different door. It keeps its own search stage, because citations, run
// metering and the no-keyless policy are Talaria's, and it runs on the
// workspace search URL or a registered search MCP server.
//
// UNSET RENDERS NOTHING. With no backend and no search URL, every agent keeps
// Hermes's own resolution, which on v0.22.4 ends at the keyless public ring
// (the anonymous tiers of Exa, Parallel, Firecrawl and Keenable). Talaria
// recommends no vendor. Once the operator picks something (a backend here,
// or a search URL that reaches the agents), the ring and Hermes's one-shot
// "rescue" through it are both switched off, unless the operator explicitly
// allowed the public tiers. A query the operator routed to their own
// provider must not fall through to a third party they never chose.
//
// THE KEY NEVER TOUCHES A CONFIG FILE. It is sealed into this setting row
// (envelope encryption, docs/ENCRYPTION.md). The render unseals it into the
// Talaria-owned fleet .env (0600) under a `TALARIA_WEB_` name and
// interpolates it into each agent service as the exact variable Hermes's
// provider reads. Hermes keeps every one of those names out of its terminal
// and execute_code children: they are `tool` entries in its
// OPTIONAL_ENV_VARS, so its provider env blocklist strips them.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use sqlx::PgPool;

/// The app_settings key.
pub const SETTING_KEY: &str = "agent_web_search";

/// One Hermes web backend Talaria offers, as Hermes v0.22.4 defines it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backend {
    /// Hermes's provider name: the `web.search_backend` value, the `NAME`
    /// in `plugins/web/<vendor>/provider.py`.
    pub name: &'static str,
    pub label: &'static str,
    /// The env var the provider reads its key from (`KEY_ENV`), when it
    /// takes one.
    pub key_env: Option<&'static str>,
    /// `supports_extract()`: it can serve `web_extract` as well as search.
    pub extract: bool,
    /// Without a key, Hermes sends this vendor's calls to the vendor's
    /// anonymous public tier instead of failing. Such a backend therefore
    /// needs a key unless the operator allowed the public tiers.
    pub public_tier: bool,
    /// The v0.22.4 image does not ship this vendor's SDK. Hermes installs it
    /// into the agent's state volume on first keyed use and asks for a
    /// restart before it loads.
    pub sdk_on_first_use: bool,
}

/// The backends Talaria offers, checked against Hermes v0.22.4's
/// `plugins/web/*/provider.py` (NAME, KEY_ENV, EXTRACT, KEYLESS) and its
/// `tools/web_tools.py` selection code.
///
/// Three registered v0.22.4 backends are left out on purpose:
/// - `ddgs`: the image does not ship the `ddgs` package, and Hermes installs
///   it only from its interactive `hermes tools` picker. Picked here, it
///   could never answer.
/// - `xai`: Grok's agentic search is a model call to x.ai on
///   `XAI_API_KEY`. That is a raw model endpoint outside Talaria's gateway.
/// - `openai-native`: it needs the Codex Responses transport and an
///   openai-codex OAuth login. Talaria agents run on the gateway's chat
///   completions, so it can never be active.
pub const BACKENDS: &[Backend] = &[
    Backend {
        name: "tavily",
        label: "Tavily",
        key_env: Some("TAVILY_API_KEY"),
        extract: true,
        public_tier: true,
        sdk_on_first_use: false,
    },
    Backend {
        name: "exa",
        label: "Exa",
        key_env: Some("EXA_API_KEY"),
        extract: true,
        public_tier: true,
        sdk_on_first_use: true,
    },
    Backend {
        name: "parallel",
        label: "Parallel",
        key_env: Some("PARALLEL_API_KEY"),
        extract: true,
        public_tier: true,
        sdk_on_first_use: true,
    },
    Backend {
        name: "firecrawl",
        label: "Firecrawl",
        key_env: Some("FIRECRAWL_API_KEY"),
        extract: true,
        public_tier: true,
        sdk_on_first_use: true,
    },
    Backend {
        name: "keenable",
        label: "Keenable",
        key_env: Some("KEENABLE_API_KEY"),
        extract: true,
        public_tier: true,
        sdk_on_first_use: false,
    },
    Backend {
        name: "perplexity",
        label: "Perplexity Search API",
        key_env: Some("PERPLEXITY_API_KEY"),
        extract: true,
        public_tier: false,
        sdk_on_first_use: false,
    },
    Backend {
        name: "brave-free",
        label: "Brave Search",
        key_env: Some("BRAVE_SEARCH_API_KEY"),
        extract: false,
        public_tier: false,
        sdk_on_first_use: false,
    },
    Backend {
        name: "searxng",
        label: "SearXNG (the search URL)",
        key_env: None,
        extract: false,
        public_tier: false,
        sdk_on_first_use: false,
    },
];

pub fn backend(name: &str) -> Option<&'static Backend> {
    BACKENDS.iter().find(|b| b.name == name)
}

/// The setting as stored. The keys ride sealed and only the render opens
/// them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentWebSearch {
    /// `web.search_backend`. None means Hermes resolves its own.
    pub backend: Option<String>,
    /// `web.extract_backend`. None means Hermes resolves its own.
    pub extract_backend: Option<String>,
    /// Vendor name to sealed key. Only vendors that are currently picked
    /// keep one; [`apply`] drops the rest.
    pub keys: BTreeMap<String, String>,
    /// Leave Hermes's keyless public ring and its rescue on even though a
    /// provider was picked.
    pub allow_public_tiers: bool,
}

impl AgentWebSearch {
    /// Read leniently: an unreadable or empty field is unset, never an
    /// error. Secret-health's leaf clear writes `""` over a key, and that
    /// reads as no key.
    pub fn from_stored(v: &Value) -> Self {
        let name = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
        };
        let keys = v
            .get("keys")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(vendor, sealed)| {
                        sealed
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(|s| (vendor.clone(), s.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            backend: name("backend"),
            extract_backend: name("extractBackend"),
            keys,
            allow_public_tiers: v
                .get("allowKeylessPublicTiers")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }
    }

    pub fn to_stored(&self) -> Value {
        json!({
            "backend": self.backend,
            "extractBackend": self.extract_backend,
            "keys": self.keys,
            "allowKeylessPublicTiers": self.allow_public_tiers,
        })
    }

    /// The vendors that are picked, search first, without repeats.
    fn picked(&self) -> Vec<&'static Backend> {
        let mut out: Vec<&'static Backend> = Vec::new();
        for name in [&self.backend, &self.extract_backend].into_iter().flatten() {
            if let Some(b) = backend(name)
                && !out.iter().any(|o| o.name == b.name)
            {
                out.push(b);
            }
        }
        out
    }
}

pub async fn read(pg: &PgPool) -> AgentWebSearch {
    AgentWebSearch::from_stored(
        &talaria_settings::get_setting(pg, SETTING_KEY, Value::Object(Map::new())).await,
    )
}

pub async fn write(pg: &PgPool, cfg: &AgentWebSearch) -> Result<(), sqlx::Error> {
    talaria_settings::set_setting(pg, SETTING_KEY, &cfg.to_stored()).await
}

/// An admin's edit. Absent fields keep what is stored.
#[derive(Debug, Default)]
pub struct Patch {
    /// Some(None) hands search back to Hermes's own resolution.
    pub backend: Option<Option<String>>,
    pub extract_backend: Option<Option<String>>,
    /// Per vendor: Some(key) replaces the stored key, None clears it.
    pub keys: Vec<(String, Option<String>)>,
    pub allow_public_tiers: Option<bool>,
}

/// A key reaches the fleet .env as one unquoted `NAME=value` line, and
/// compose reads that file with interpolation and inline comments. So the
/// value is held to what vendor keys are made of: no whitespace, no quotes,
/// no `#`, no `$`.
fn key_chars_ok(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._~:+/=-".contains(c))
}

pub const MAX_KEY_LEN: usize = 500;

/// Apply `patch` to `cur`. Returns the setting to store, or the sentence
/// the admin reads. `search_url_set` says whether the workspace has a search
/// URL (SearXNG needs one). `seal` seals a new key.
pub fn apply(
    cur: &AgentWebSearch,
    patch: &Patch,
    search_url_set: bool,
    seal: impl Fn(&str) -> Result<String, String>,
) -> Result<AgentWebSearch, String> {
    let names: Vec<&str> = BACKENDS.iter().map(|b| b.name).collect();
    let pick = |v: &Option<String>, extract: bool| -> Result<Option<String>, String> {
        let Some(name) = v.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        let b = backend(name).ok_or_else(|| {
            format!(
                "\"{name}\" is not a web backend Talaria offers: pick one of {}, or leave it empty for Hermes's own default",
                names.join(", ")
            )
        })?;
        if extract && !b.extract {
            let capable: Vec<&str> = BACKENDS
                .iter()
                .filter(|b| b.extract)
                .map(|b| b.name)
                .collect();
            return Err(format!(
                "{} cannot extract pages: pick one of {}, or leave page extraction to Hermes",
                b.label,
                capable.join(", ")
            ));
        }
        Ok(Some(b.name.to_string()))
    };

    let mut next = cur.clone();
    if let Some(v) = &patch.backend {
        next.backend = pick(v, false)?;
    }
    if let Some(v) = &patch.extract_backend {
        next.extract_backend = pick(v, true)?;
    }
    if let Some(allow) = patch.allow_public_tiers {
        next.allow_public_tiers = allow;
    }
    let picked = next.picked();
    for (vendor, key) in &patch.keys {
        let Some(b) = backend(vendor).filter(|b| b.key_env.is_some()) else {
            return Err(format!("keys.{vendor}: not a web backend that takes a key"));
        };
        match key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
            None => {
                next.keys.remove(b.name);
            }
            Some(k) => {
                if !picked.iter().any(|p| p.name == b.name) {
                    return Err(format!(
                        "keys.{vendor}: {} is not picked, so its key would never reach an agent",
                        b.label
                    ));
                }
                if k.len() > MAX_KEY_LEN {
                    return Err(format!(
                        "keys.{vendor}: a key is at most {MAX_KEY_LEN} characters"
                    ));
                }
                if !key_chars_ok(k) {
                    return Err(format!(
                        "keys.{vendor}: a key may hold only letters, digits and . _ ~ : + / = -"
                    ));
                }
                next.keys.insert(b.name.to_string(), seal(k)?);
            }
        }
    }
    // A vendor that is no longer picked keeps no key: a secret nothing
    // reads is still a secret to leak.
    next.keys
        .retain(|vendor, _| picked.iter().any(|p| p.name == vendor));

    for b in &picked {
        if b.key_env.is_some() && !next.keys.contains_key(b.name) {
            if !b.public_tier {
                return Err(format!("{} needs an API key", b.label));
            }
            if !next.allow_public_tiers {
                return Err(format!(
                    "{} needs an API key. Without one Hermes would send its calls to {}'s anonymous public tier, which this workspace has not allowed",
                    b.label, b.label
                ));
            }
        }
        if b.name == "searxng" && !search_url_set {
            return Err(
                "SearXNG searches the workspace search URL, and none is set: set the search URL first"
                    .into(),
            );
        }
    }
    Ok(next)
}

/// What one render hands every agent.
#[derive(Debug, Default, PartialEq)]
pub struct WebRender {
    /// Keys for the agent's config.yaml `web:` block, in emit order.
    pub web: Vec<(&'static str, Value)>,
    /// (Hermes env var, sealed key) for every picked vendor holding a key.
    pub keys: Vec<(&'static str, String)>,
    pub warnings: Vec<String>,
}

/// The render, pure. `searxng_reaches_agents` says whether the workspace
/// search URL is rendered into the agents as `SEARXNG_URL`: a URL the agents
/// cannot reach chooses nothing for them, so it does not switch the public
/// tiers off.
pub fn render(cfg: &AgentWebSearch, searxng_reaches_agents: bool) -> WebRender {
    let mut out = WebRender::default();
    let known = |name: &Option<String>| name.as_deref().and_then(backend);
    let search = known(&cfg.backend);
    let extract = known(&cfg.extract_backend);
    if let Some(b) = search {
        out.web.push(("search_backend", json!(b.name)));
    }
    if let Some(b) = extract {
        out.web.push(("extract_backend", json!(b.name)));
    }
    for (name, which) in [(&cfg.backend, "search"), (&cfg.extract_backend, "extract")] {
        if let Some(n) = name
            && backend(n).is_none()
        {
            out.warnings.push(format!(
                "agent web search: the stored {which} backend \"{n}\" is not one this build offers, so it is not rendered; pick one on Admin → Search"
            ));
        }
    }
    for b in cfg.picked() {
        if let (Some(env), Some(sealed)) = (b.key_env, cfg.keys.get(b.name)) {
            out.keys.push((env, sealed.clone()));
        }
        if b.name == "searxng" && !searxng_reaches_agents {
            out.warnings.push(
                "agent web search: SearXNG is picked, but the workspace search URL does not reach agent containers, so their searches will fail; give it an address the agents can reach"
                    .into(),
            );
        }
    }
    let chosen = search.is_some() || extract.is_some() || searxng_reaches_agents;
    if chosen && !cfg.allow_public_tiers {
        out.web.push(("keyless_fallback", json!(false)));
        out.web.push(("keyless_rescue", json!(false)));
    }
    out
}

/// Merge the render into an agent's config, keeping whatever else its own
/// `web:` block says. Nothing to render leaves the config untouched.
pub fn merge_into(config: &mut Map<String, Value>, web: &[(&'static str, Value)]) {
    if web.is_empty() {
        return;
    }
    let mut block = config
        .get("web")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (k, v) in web {
        block.insert((*k).to_string(), v.clone());
    }
    config.insert("web".into(), Value::Object(block));
}

/// Every fleet .env line this module owns starts with this.
pub const FLEET_ENV_PREFIX: &str = "TALARIA_WEB_";
const FLEET_ENV_HEADER: &str = "# agent web search keys — Talaria-owned";

/// The fleet .env name that holds the value for Hermes's `key_env`.
pub fn fleet_env_var(key_env: &str) -> String {
    format!("{FLEET_ENV_PREFIX}{key_env}")
}

/// The fleet .env as it should read: every line this module wrote before
/// (and its header) removed, then one `TALARIA_WEB_<VAR>=<key>` per
/// delivered key. Nothing delivered leaves no trace.
pub fn fleet_env_with(current: &str, delivered: &[(&str, String)]) -> String {
    let kept: Vec<&str> = current
        .lines()
        .filter(|l| !l.starts_with(FLEET_ENV_PREFIX) && *l != FLEET_ENV_HEADER)
        .collect();
    let mut next = kept.join("\n");
    while next.ends_with('\n') {
        next.pop();
    }
    if !delivered.is_empty() {
        next.push('\n');
        next.push_str(FLEET_ENV_HEADER);
        for (var, value) in delivered {
            next.push('\n');
            next.push_str(&format!("{}={value}", fleet_env_var(var)));
        }
    }
    if !next.is_empty() {
        next.push('\n');
    }
    next
}

/// A value safe to write as one unquoted .env line. The render checks it
/// again after unsealing, so a row written around [`apply`] still cannot
/// inject a line.
pub fn fleet_env_value_ok(value: &str) -> bool {
    value.len() <= MAX_KEY_LEN && key_chars_ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seal(k: &str) -> Result<String, String> {
        Ok(format!("sealed:{k}"))
    }

    fn cfg(backend: Option<&str>, extract: Option<&str>, keys: &[(&str, &str)]) -> AgentWebSearch {
        AgentWebSearch {
            backend: backend.map(String::from),
            extract_backend: extract.map(String::from),
            keys: keys
                .iter()
                .map(|(v, k)| ((*v).to_string(), (*k).to_string()))
                .collect(),
            allow_public_tiers: false,
        }
    }

    /// The catalog against Hermes v0.22.4, every member: each provider's
    /// NAME, KEY_ENV and EXTRACT from `plugins/web/<vendor>/provider.py`,
    /// and KEYLESS (or firecrawl's explicit-selection keyless cloud) for the
    /// public tier. A Hermes bump that renames a provider or its key fails
    /// here, not in an agent's transcript.
    #[test]
    fn the_catalog_matches_hermes_v0_22_4() {
        let got: Vec<(&str, Option<&str>, bool, bool)> = BACKENDS
            .iter()
            .map(|b| (b.name, b.key_env, b.extract, b.public_tier))
            .collect();
        assert_eq!(
            got,
            vec![
                ("tavily", Some("TAVILY_API_KEY"), true, true),
                ("exa", Some("EXA_API_KEY"), true, true),
                ("parallel", Some("PARALLEL_API_KEY"), true, true),
                ("firecrawl", Some("FIRECRAWL_API_KEY"), true, true),
                ("keenable", Some("KEENABLE_API_KEY"), true, true),
                ("perplexity", Some("PERPLEXITY_API_KEY"), true, false),
                ("brave-free", Some("BRAVE_SEARCH_API_KEY"), false, false),
                ("searxng", None, false, false),
            ]
        );
        for left_out in ["ddgs", "xai", "openai-native"] {
            assert!(backend(left_out).is_none(), "{left_out} is offered");
        }
    }

    #[test]
    fn unset_renders_nothing() {
        assert_eq!(
            render(&AgentWebSearch::default(), false),
            WebRender::default()
        );
    }

    #[test]
    fn a_search_url_alone_switches_the_public_tiers_off() {
        let r = render(&AgentWebSearch::default(), true);
        assert_eq!(
            r.web,
            vec![
                ("keyless_fallback", json!(false)),
                ("keyless_rescue", json!(false))
            ]
        );
        assert!(r.keys.is_empty());
        let allowed = AgentWebSearch {
            allow_public_tiers: true,
            ..Default::default()
        };
        assert_eq!(render(&allowed, true), WebRender::default());
    }

    #[test]
    fn a_picked_backend_renders_its_name_its_key_and_no_public_tiers() {
        let r = render(&cfg(Some("tavily"), None, &[("tavily", "sealed:k")]), false);
        assert_eq!(
            r.web,
            vec![
                ("search_backend", json!("tavily")),
                ("keyless_fallback", json!(false)),
                ("keyless_rescue", json!(false)),
            ]
        );
        assert_eq!(r.keys, vec![("TAVILY_API_KEY", "sealed:k".to_string())]);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn search_and_extract_each_render_with_their_own_key() {
        let r = render(
            &cfg(
                Some("brave-free"),
                Some("firecrawl"),
                &[("brave-free", "sealed:b"), ("firecrawl", "sealed:f")],
            ),
            false,
        );
        assert_eq!(
            r.web,
            vec![
                ("search_backend", json!("brave-free")),
                ("extract_backend", json!("firecrawl")),
                ("keyless_fallback", json!(false)),
                ("keyless_rescue", json!(false)),
            ]
        );
        assert_eq!(
            r.keys,
            vec![
                ("BRAVE_SEARCH_API_KEY", "sealed:b".to_string()),
                ("FIRECRAWL_API_KEY", "sealed:f".to_string()),
            ]
        );
        // One vendor for both capabilities: one key, rendered once.
        let same = render(
            &cfg(Some("exa"), Some("exa"), &[("exa", "sealed:e")]),
            false,
        );
        assert_eq!(same.keys, vec![("EXA_API_KEY", "sealed:e".to_string())]);
    }

    #[test]
    fn allowed_public_tiers_leave_hermes_defaults_alone() {
        let mut c = cfg(Some("exa"), None, &[]);
        c.allow_public_tiers = true;
        assert_eq!(render(&c, true).web, vec![("search_backend", json!("exa"))]);
    }

    #[test]
    fn searxng_that_cannot_reach_the_agents_warns() {
        let r = render(&cfg(Some("searxng"), None, &[]), false);
        assert_eq!(r.web[0], ("search_backend", json!("searxng")));
        assert_eq!(r.warnings.len(), 1);
        assert!(
            render(&cfg(Some("searxng"), None, &[]), true)
                .warnings
                .is_empty()
        );
    }

    #[test]
    fn an_unknown_stored_backend_is_not_rendered() {
        let r = render(&cfg(Some("ddgs"), None, &[]), false);
        assert!(r.web.is_empty());
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn apply_picks_seals_and_validates() {
        let base = AgentWebSearch::default();
        let picked = apply(
            &base,
            &Patch {
                backend: Some(Some("tavily".into())),
                keys: vec![("tavily".into(), Some(" tvly-abc123 ".into()))],
                ..Default::default()
            },
            false,
            seal,
        )
        .unwrap();
        assert_eq!(picked.backend.as_deref(), Some("tavily"));
        assert_eq!(
            picked.keys.get("tavily").map(String::as_str),
            Some("sealed:tvly-abc123")
        );

        // Absent key keeps the stored one.
        let kept = apply(&picked, &Patch::default(), false, seal).unwrap();
        assert_eq!(kept, picked);

        // Unknown names, and a search-only backend for extraction.
        let bad = |p: Patch| apply(&base, &p, true, seal).unwrap_err();
        assert!(
            bad(Patch {
                backend: Some(Some("ddgs".into())),
                ..Default::default()
            })
            .contains("ddgs")
        );
        assert!(
            bad(Patch {
                extract_backend: Some(Some("brave-free".into())),
                ..Default::default()
            })
            .contains("cannot extract")
        );
        // A key for a vendor nobody picked.
        assert!(
            bad(Patch {
                keys: vec![("exa".into(), Some("k".into()))],
                ..Default::default()
            })
            .contains("not picked")
        );
        // A key that would break the .env line.
        assert!(
            bad(Patch {
                backend: Some(Some("exa".into())),
                keys: vec![("exa".into(), Some("a b".into()))],
                ..Default::default()
            })
            .contains("only letters")
        );
    }

    #[test]
    fn a_keyed_backend_without_a_key_needs_the_public_tiers_or_a_key() {
        let base = AgentWebSearch::default();
        let p = |allow: Option<bool>| Patch {
            backend: Some(Some("firecrawl".into())),
            allow_public_tiers: allow,
            ..Default::default()
        };
        assert!(
            apply(&base, &p(None), false, seal)
                .unwrap_err()
                .contains("public tier")
        );
        assert!(apply(&base, &p(Some(true)), false, seal).is_ok());
        // Perplexity and Brave have no anonymous tier at all.
        for name in ["perplexity", "brave-free"] {
            let e = apply(
                &base,
                &Patch {
                    backend: Some(Some(name.into())),
                    allow_public_tiers: Some(true),
                    ..Default::default()
                },
                false,
                seal,
            )
            .unwrap_err();
            assert!(e.ends_with("needs an API key"), "{name}: {e}");
        }
    }

    #[test]
    fn searxng_needs_the_search_url() {
        let p = Patch {
            backend: Some(Some("searxng".into())),
            ..Default::default()
        };
        assert!(apply(&AgentWebSearch::default(), &p, false, seal).is_err());
        assert!(apply(&AgentWebSearch::default(), &p, true, seal).is_ok());
    }

    #[test]
    fn switching_vendors_drops_the_old_key() {
        let cur = cfg(Some("tavily"), None, &[("tavily", "sealed:t")]);
        let next = apply(
            &cur,
            &Patch {
                backend: Some(Some("exa".into())),
                keys: vec![("exa".into(), Some("exa-key".into()))],
                ..Default::default()
            },
            false,
            seal,
        )
        .unwrap();
        assert_eq!(next.keys.keys().collect::<Vec<_>>(), vec!["exa"]);
        // Back to Hermes's default: nothing picked, nothing kept.
        let cleared = apply(
            &next,
            &Patch {
                backend: Some(None),
                ..Default::default()
            },
            false,
            seal,
        )
        .unwrap();
        assert_eq!(cleared, AgentWebSearch::default());
    }

    #[test]
    fn the_stored_shape_round_trips_and_reads_leniently() {
        let c = cfg(Some("exa"), Some("firecrawl"), &[("exa", "v2:1:a:b:c")]);
        assert_eq!(AgentWebSearch::from_stored(&c.to_stored()), c);
        assert_eq!(
            AgentWebSearch::from_stored(&json!(null)),
            AgentWebSearch::default()
        );
        // A cleared key ("" from secret-health) is no key.
        let cleared =
            AgentWebSearch::from_stored(&json!({ "backend": "exa", "keys": { "exa": "" } }));
        assert!(cleared.keys.is_empty());
    }

    #[test]
    fn merge_keeps_the_agents_own_web_keys() {
        let mut config = json!({ "web": { "extract_char_limit": 9000, "search_backend": "ddgs" } })
            .as_object()
            .cloned()
            .unwrap();
        merge_into(&mut config, &[("search_backend", json!("exa"))]);
        assert_eq!(
            Value::Object(config.clone()),
            json!({ "web": { "extract_char_limit": 9000, "search_backend": "exa" } })
        );
        let before = config.clone();
        merge_into(&mut config, &[]);
        assert_eq!(config, before);
    }

    #[test]
    fn the_fleet_env_lines_are_rewritten_whole() {
        let base = "HERMES_KEY_ADA=abc\nTALARIA_AGENT_KEY=xyz\n";
        let one = fleet_env_with(base, &[("TAVILY_API_KEY", "tvly-1".into())]);
        assert_eq!(
            one,
            "HERMES_KEY_ADA=abc\nTALARIA_AGENT_KEY=xyz\n# agent web search keys — Talaria-owned\nTALARIA_WEB_TAVILY_API_KEY=tvly-1\n"
        );
        // Idempotent, and a switch replaces rather than accumulates.
        assert_eq!(
            fleet_env_with(&one, &[("TAVILY_API_KEY", "tvly-1".into())]),
            one
        );
        let switched = fleet_env_with(&one, &[("EXA_API_KEY", "e".into())]);
        assert!(!switched.contains("TAVILY"));
        assert_eq!(switched.matches(FLEET_ENV_HEADER).count(), 1);
        // Cleared: back to the file as it was.
        assert_eq!(fleet_env_with(&one, &[]), base);
        assert_eq!(fleet_env_with("", &[]), "");
    }
}
