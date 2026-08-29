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

/// fleet.json — the render OUTPUT table of {model, url, key} per agent. Only
/// `model` is read here; the keys never leave the fleet plane.
#[derive(serde::Deserialize)]
struct ManifestEntry {
    model: String,
}

/// readManifest: a missing or unparseable manifest is an EMPTY fleet, and says
/// so — it never falls back to invented agents (gateway.ts).
async fn read_manifest() -> Vec<ManifestEntry> {
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
