// The fleet engine family — the registry brain, rendering, reconciliation, docker, preflight, federation.
pub mod brain;
pub mod cascade;
pub mod create;
pub mod docker;
pub mod federate;
pub mod layout;
pub mod preflight;
pub mod reconcile;
pub mod render;

// The fleet as AGENTS (not raw gateway models) — port of the read side of
// ui/src/server/fleet-agents.ts + the access gate from users.ts that its one
// route applies. The bridge's /v1/models includes one entry per tier
// (`<base>-<alias>`), so consumers that mean "agents" must filter to
// definition models and pick tiers up separately — that filtering lives here.

use crate::users::personal_assistant_owners;
use sqlx::PgPool;

#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentModel {
    pub id: String,
    /// Display label — the leading segment of the id, capitalised.
    pub label: String,
    /// Role remainder of the id.
    pub role: String,
}

/// Split an agent id into its display halves: "<slug>-<role>"
/// (gateway.ts describeAgent). An empty leading segment (a id like "-lead")
/// keeps the WHOLE id as the label — JS's falsy '' falls to the `: id` arm.
pub fn describe_agent(id: &str) -> AgentModel {
    let (first, rest) = match id.split_once('-') {
        Some((f, r)) => (f, r),
        None => (id, ""),
    };
    let label = if first.is_empty() {
        id.to_string()
    } else {
        // charAt(0).toUpperCase() — one code point; agent ids are ASCII.
        let mut c = first.chars();
        match c.next() {
            Some(h) => h.to_uppercase().collect::<String>() + c.as_str(),
            None => first.to_string(),
        }
    };
    AgentModel {
        id: id.to_string(),
        label,
        // rest.join(' ') — empty when the id has no dash at all.
        role: rest.replace('-', " "),
    }
}

/// fleet.json — the render OUTPUT table of {model, url, key} per agent. The
/// url/key halves are the comms transport's (gateway/fleet_chat.rs); here only
/// `model` is read, and the keys never leave the fleet plane.
#[derive(serde::Deserialize)]
pub struct ManifestEntry {
    pub model: String,
    pub url: String,
    pub key: Option<String>,
}

/// readManifest: a missing or unparseable manifest is an EMPTY fleet, and says
/// so — it never falls back to invented agents (gateway.ts).
pub async fn read_manifest() -> Vec<ManifestEntry> {
    let Ok(raw) =
        tokio::fs::read_to_string(crate::gateway::provider::fleet_dir().join("fleet.json")).await
    else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<ManifestEntry>>(&raw).unwrap_or_default()
}

/// The fleet from the manifest — base models only; tier entries like
/// "<base>-<alias>" are hidden from the picker (gateway.ts listAgents).
pub async fn list_agents() -> Vec<AgentModel> {
    let manifest = read_manifest().await;
    if manifest.is_empty() {
        return Vec::new();
    }
    let ids: std::collections::HashSet<&str> = manifest.iter().map(|m| m.model.as_str()).collect();
    let is_tier = |id: &str| match id.rfind('-') {
        Some(i) if i > 0 => ids.contains(&id[..i]),
        _ => false,
    };
    // [...new Set(...)] in MANIFEST order — the picker's order is the
    // manifest's order, first occurrence wins on a duplicate.
    let mut seen = std::collections::HashSet::new();
    manifest
        .iter()
        .map(|m| m.model.as_str())
        .filter(|id| !is_tier(id) && seen.insert(id.to_string()))
        .map(describe_agent)
        .collect()
}

#[derive(Debug, serde::Serialize)]
pub struct FleetAgentEntry {
    #[serde(flatten)]
    pub agent: AgentModel,
    /// Model tiers requestable for this agent (alias names; main is implicit).
    pub tiers: Vec<String>,
}

/// The agents that EXIST, which is a question only `agent_defs` can answer
/// (fleet-agents.ts listFleetAgents). fleet/fleet.json is a render OUTPUT —
/// the transport table of where each agent's gateway lives — not a roster:
/// treating it as one meant a manifest left on disk resurrected agents the
/// database no longer had. The filter is unconditional; an empty definition
/// table means NONE, not "unknown".
pub async fn list_fleet_agents(pg: &PgPool) -> Result<Vec<FleetAgentEntry>, sqlx::Error> {
    let agents = list_agents().await;
    let rows: Vec<(String, Option<serde_json::Value>)> = sqlx::query_as(
        "select d.model, v.config \
         from agent_defs d \
         left join agent_versions v on v.agent_id = d.id and v.version = d.current_version \
         where d.enabled",
    )
    .fetch_all(pg)
    .await?;
    // config?.aliases?.map(a => a.name) ?? [] — a missing config, a config
    // without aliases, and a non-array aliases all degrade to no tiers.
    let tiers_of = |config: &Option<serde_json::Value>| -> Vec<String> {
        config
            .as_ref()
            .and_then(|c| c.get("aliases"))
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.get("name").and_then(|n| n.as_str()))
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    };
    let by_model: std::collections::HashMap<String, Vec<String>> = rows
        .into_iter()
        .map(|(model, config)| {
            let t = tiers_of(&config);
            (model, t)
        })
        .collect();
    Ok(agents
        .into_iter()
        .filter_map(|a| {
            by_model.get(&a.id).map(|tiers| FleetAgentEntry {
                agent: a,
                tiers: tiers.clone(),
            })
        })
        .collect())
}

/// Validate a tier for an agent; returns the routed gateway model id
/// (fleet-agents.ts routedModelFor). No tier → the agent's main model id as-is;
/// a tier the agent does not declare → None (the caller rejects the typo
/// loudly rather than recording an unattributable, unpriceable usage row).
pub async fn routed_model_for(
    pg: &PgPool,
    agent_model: &str,
    tier: Option<&str>,
) -> Result<Option<String>, sqlx::Error> {
    let Some(tier) = tier.filter(|t| !t.is_empty()) else {
        return Ok(Some(agent_model.to_string()));
    };
    let agents = list_fleet_agents(pg).await?;
    let Some(a) = agents.iter().find(|x| x.agent.id == agent_model) else {
        return Ok(None);
    };
    if !a.tiers.iter().any(|t| t == tier) {
        return Ok(None);
    }
    Ok(Some(format!("{agent_model}-{tier}")))
}

// ── Per-agent access (users.ts allowedAgents / canUseAgent / usableAgentGate) ─

/// A member's agent access: 'all', or an explicit allow-list. No rows at all
/// means 'all' — agent access is grant-NARROWING, not grant-by-default.
pub enum AgentAccess {
    All,
    List(Vec<String>),
}

pub async fn allowed_agents(
    pg: &PgPool,
    user_id: &str,
    role: &str,
) -> Result<AgentAccess, sqlx::Error> {
    if role == "admin" {
        return Ok(AgentAccess::All);
    }
    let rows: Vec<(String,)> =
        sqlx::query_as("select agent_model from user_agent_access where user_id = $1::uuid")
            .bind(user_id)
            .fetch_all(pg)
            .await?;
    if rows.is_empty() {
        return Ok(AgentAccess::All);
    }
    Ok(AgentAccess::List(rows.into_iter().map(|(m,)| m).collect()))
}

pub fn can_use_agent(access: &AgentAccess, model: &str) -> bool {
    match access {
        AgentAccess::All => true,
        AgentAccess::List(l) => l.iter().any(|m| m == model),
    }
}

/// The usable-agent predicate (users.ts usableAgentGate): a personal assistant
/// is only ever visible to its owner, and everything else rides the member's
/// access resolution.
pub async fn usable_agent_gate(
    pg: &PgPool,
    user_id: &str,
    role: &str,
) -> Result<impl Fn(&str) -> bool + use<>, sqlx::Error> {
    let access = allowed_agents(pg, user_id, role).await?;
    let owners = personal_assistant_owners(pg).await?;
    let user_id = user_id.to_string();
    Ok(move |model: &str| {
        // someone else's personal assistant — never visible
        if owners
            .get(model)
            .is_some_and(|owner| owner.as_str() != user_id)
        {
            return false;
        }
        can_use_agent(&access, model)
    })
}

// ── The ops overview (fleet.ts getFleetOverview) ─────────────────────────────

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetAgentStat {
    pub id: String,
    pub label: String,
    pub role: String,
    pub status: String,
    pub last_seen: Option<String>,
    pub last_activity: Option<String>,
    pub conversations: i64,
    pub messages: i64,
    pub last_used: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetTotals {
    pub agents: usize,
    pub online: usize,
    pub conversations: i64,
    pub messages: i64,
    pub active_today: usize,
}

#[derive(Debug, serde::Serialize)]
pub struct FleetOverview {
    pub agents: Vec<FleetAgentStat>,
    pub totals: FleetTotals,
}

/// Owned fleet ops data — agents + Talaria-native usage (getFleetOverview).
/// Heartbeats are the ideal liveness signal, but most agents don't heartbeat
/// to Talaria yet; container reality (the roster status dots' own source)
/// fills the gap, so the "online" count can never disagree with the green
/// dots on the agents page: running container ⇒ online (`idle` when the
/// registry says offline). `now` is epoch ms (house rule).
pub async fn get_fleet_overview(pg: &PgPool, now: i64) -> Result<FleetOverview, sqlx::Error> {
    let agents = list_fleet_agents(pg).await?;
    // Seed the registry from the fleet so every agent shows (offline until it
    // heartbeats to Talaria), then read owned status.
    let seed: Vec<String> = agents.iter().map(|a| a.agent.id.clone()).collect();
    crate::agents_registry::seed_fleet_names(pg, &seed).await?;
    let registry = crate::agents_registry::registry_by_name(pg, now).await?;

    // defs (enabled only) → container reality, keyed by the def's MODEL (the
    // agent id). A defs read that fails reads as "no containers" (TS's
    // `.catch(() => [])`).
    let defs: Vec<(String, String)> =
        sqlx::query_as("select model, department from agent_defs where enabled")
            .fetch_all(pg)
            .await
            .unwrap_or_default();
    let containers = if defs.is_empty() {
        Vec::new()
    } else {
        crate::fleet::docker::container_status(
            &defs.iter().map(|(_, d)| d.clone()).collect::<Vec<_>>(),
        )
        .await
        .unwrap_or_default()
    };
    let by_dept: std::collections::HashMap<&str, &crate::fleet::docker::AgentContainers> =
        containers
            .iter()
            .map(|c| (c.department.as_str(), c))
            .collect();
    let container_up: std::collections::HashMap<&str, bool> = defs
        .iter()
        .map(|(model, dept)| {
            let running = by_dept
                .get(dept.as_str())
                .and_then(|c| c.managed.as_ref())
                .is_some_and(|m| m.state == "running");
            (model.as_str(), running)
        })
        .collect();

    // Fleet-wide usage (all users) per agent — the ops/maintainer view.
    let rows: Vec<(String, i32, i32, Option<i64>)> = sqlx::query_as(
        "select c.agent_model as model, \
           count(distinct c.id)::int as conversations, \
           count(m.id)::int as messages, \
           (trunc(extract(epoch from max(c.updated_at)) * 1000))::bigint as last_used \
         from conversations c \
         left join messages m on m.conversation_id = c.id \
         group by c.agent_model",
    )
    .fetch_all(pg)
    .await?;
    let by_model: std::collections::HashMap<&str, (i32, i32, Option<i64>)> = rows
        .iter()
        .map(|(m, c, msg, u)| (m.as_str(), (*c, *msg, *u)))
        .collect();

    let iso = crate::agent_auth::epoch_ms_to_iso;
    let stats: Vec<FleetAgentStat> = agents
        .iter()
        .map(|a| {
            let reg = registry.get(&a.agent.id);
            // reg?.status ?? 'offline'
            let reg_status = reg.map(|r| r.status.wire()).unwrap_or("offline");
            let status = if reg_status == "offline"
                && container_up
                    .get(a.agent.id.as_str())
                    .copied()
                    .unwrap_or(false)
            {
                "idle"
            } else {
                reg_status
            };
            let (conversations, messages, last_used) = by_model
                .get(a.agent.id.as_str())
                .cloned()
                .unwrap_or((0, 0, None));
            FleetAgentStat {
                id: a.agent.id.clone(),
                label: a.agent.label.clone(),
                role: a.agent.role.clone(),
                status: status.to_string(),
                last_seen: reg.and_then(|r| r.last_seen).map(iso),
                last_activity: reg.and_then(|r| r.last_activity.clone()),
                conversations: conversations as i64,
                messages: messages as i64,
                last_used: last_used.map(iso),
            }
        })
        .collect();

    let day_ago = now - 24 * 60 * 60 * 1000;
    let totals = FleetTotals {
        agents: stats.len(),
        online: stats.iter().filter(|s| s.status != "offline").count(),
        conversations: stats.iter().map(|s| s.conversations).sum(),
        messages: stats.iter().map(|s| s.messages).sum(),
        active_today: stats
            .iter()
            .filter(|s| {
                by_model
                    .get(s.id.as_str())
                    .and_then(|(_, _, u)| *u)
                    .is_some_and(|used| used > day_ago)
            })
            .count(),
    };
    Ok(FleetOverview {
        agents: stats,
        totals,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_agent_splits_like_the_js() {
        let a = describe_agent("coord-lead-ops");
        assert_eq!(a.id, "coord-lead-ops");
        assert_eq!(a.label, "Coord");
        assert_eq!(a.role, "lead ops");
        // No dash: whole id is the label, role empty.
        let b = describe_agent("solo");
        assert_eq!(b.label, "Solo");
        assert_eq!(b.role, "");
        // Empty leading segment: JS's falsy '' falls to the whole id.
        let c = describe_agent("-lead");
        assert_eq!(c.label, "-lead");
        assert_eq!(c.role, "lead");
    }

    #[test]
    fn access_gate_is_owner_aware() {
        let all = AgentAccess::All;
        assert!(can_use_agent(&all, "anything"));
        let list = AgentAccess::List(vec!["a".into(), "b".into()]);
        assert!(can_use_agent(&list, "a"));
        assert!(!can_use_agent(&list, "c"));
    }
}
