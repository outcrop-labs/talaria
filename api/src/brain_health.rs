// Brain routability — is each agent's configured model still servable?
// Provider pools churn under no-train routing (a model can drop out of the US
// pool mid-day); when an agent's rendered model loses its route, chats freeze
// silently. This probes every enabled agent's config targets against the
// gateway registry (llm_endpoints) so the failure surfaces on /agents and in
// alerts instead of as a hung reply. Port of ui/src/server/brain-health.ts.

use sqlx::PgPool;

use crate::gateway::registry::list_endpoints;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BrainTarget {
    pub kind: &'static str,
    /// Tier alias name — present only when kind === 'tier' (TS's conditional
    /// spread keeps the key absent otherwise, and so does this Option).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub endpoint: String,
    pub model: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBrainHealth {
    /// The agent's fleet model id (agent_defs.model).
    pub agent: String,
    pub display_name: String,
    /// Main brain routable? Tiers/fallbacks degrade; the main brain freezes
    /// chat. No main target at all = unroutable: nothing to serve chat with.
    pub ok: bool,
    pub targets: Vec<BrainTarget>,
}

static CACHE: std::sync::LazyLock<tokio::sync::Mutex<Option<(i64, Vec<AgentBrainHealth>)>>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(None));
const TTL_MS: i64 = 30_000;

/// One config target as read from the latest version's jsonb — endpoint and
/// model only; nothing else about the target matters to routability.
struct RawTarget<'a> {
    endpoint: &'a str,
    model: &'a str,
}

fn raw_target(v: &serde_json::Value) -> Option<RawTarget<'_>> {
    Some(RawTarget {
        endpoint: v.get("endpoint")?.as_str()?,
        model: v.get("model")?.as_str()?,
    })
}

/// Routability of every enabled agent's brain targets (30s cache).
pub async fn fleet_brain_health(pg: &PgPool) -> Result<Vec<AgentBrainHealth>, sqlx::Error> {
    {
        let cache = CACHE.lock().await;
        if let Some((at, value)) = cache.as_ref()
            && now_ms() - at < TTL_MS
        {
            return Ok(value.clone());
        }
    }
    let (defs, endpoints) = tokio::join!(
        sqlx::query_as::<_, (String, String, bool, Option<serde_json::Value>)>(
            // The config is the MAX version's, not current_version's — TS's
            // listAgentDefs resolves `latest` with `distinct on (agent_id)
            // … order by version desc` and never consults current_version.
            // The two agree while the pointer is honest; a dangling
            // current_version (a version row deleted underneath it) is
            // tolerated by TS and must be here too, or a def like that reads
            // as "no main model configured".
            "select d.model, d.display_name, d.enabled, v.config \
             from agent_defs d \
             left join lateral (select config from agent_versions \
                                where agent_id = d.id order by version desc limit 1) v on true \
             order by d.slug asc"
        )
        .fetch_all(pg),
        list_endpoints(pg)
    );
    let defs = defs?;
    let endpoints = endpoints?;
    let by_name: std::collections::HashMap<&str, &crate::gateway::registry::LlmEndpoint> =
        endpoints.iter().map(|e| (e.name.as_str(), e)).collect();

    // probe(kind, target, name?) — ok iff the endpoint exists AND lists the
    // model; the reason names which half failed.
    let probe = |kind: &'static str, t: &RawTarget<'_>, name: Option<String>| -> BrainTarget {
        let ep = by_name.get(t.endpoint);
        let ok = ep.is_some_and(|e| e.models.iter().any(|m| m == t.model));
        BrainTarget {
            kind,
            name,
            endpoint: t.endpoint.to_string(),
            model: t.model.to_string(),
            ok,
            reason: if ok {
                None
            } else if ep.is_some() {
                Some("model not on endpoint")
            } else {
                Some("endpoint missing")
            },
        }
    };

    let mut value = Vec::new();
    for (model, display_name, enabled, config) in &defs {
        if !enabled {
            continue;
        }
        let cfg = config.as_ref();
        let mut targets = Vec::new();
        if let Some(main) = cfg.and_then(|c| c.get("main")).and_then(raw_target) {
            targets.push(probe("main", &main, None));
        }
        if let Some(aliases) = cfg
            .and_then(|c| c.get("aliases"))
            .and_then(|a| a.as_array())
        {
            for a in aliases {
                if let Some(t) = raw_target(a) {
                    let name = a.get("name").and_then(|n| n.as_str()).map(String::from);
                    targets.push(probe("tier", &t, name));
                }
            }
        }
        if let Some(fallbacks) = cfg
            .and_then(|c| c.get("fallbacks"))
            .and_then(|f| f.as_array())
        {
            for f in fallbacks {
                if let Some(t) = raw_target(f) {
                    targets.push(probe("fallback", &t, None));
                }
            }
        }
        let ok = if targets.iter().any(|t| t.kind == "main") {
            targets.iter().all(|t| t.kind != "main" || t.ok)
        } else {
            false
        };
        value.push(AgentBrainHealth {
            agent: model.clone(),
            display_name: display_name.clone(),
            ok,
            targets,
        });
    }
    *CACHE.lock().await = Some((now_ms(), value.clone()));
    Ok(value)
}
