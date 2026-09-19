// Fleet agent registry — Talaria owns this. Agents register once and
// heartbeat to keep last_seen fresh; status is derived from heartbeat
// recency so a crashed agent reads offline.

use sqlx::PgPool;

/// Heartbeat freshness: seen within this window ⇒ live; else offline.
pub const FRESH_MS: i64 = 90_000;

/// The registry's status vocabulary. Serialized lowercase, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Offline,
    Idle,
    Busy,
    Error,
}

impl AgentStatus {
    fn parse(s: &str) -> AgentStatus {
        match s {
            "idle" => AgentStatus::Idle,
            "busy" => AgentStatus::Busy,
            "error" => AgentStatus::Error,
            _ => AgentStatus::Offline,
        }
    }
    fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Offline => "offline",
            AgentStatus::Idle => "idle",
            AgentStatus::Busy => "busy",
            AgentStatus::Error => "error",
        }
    }
    /// The wire spelling for the fleet overview's `status`.
    pub fn wire(&self) -> &'static str {
        self.as_str()
    }
}

#[derive(Debug, Clone)]
pub struct RegistryAgent {
    pub id: String,
    pub name: String,
    pub role: String,
    pub status: AgentStatus,
    /// epoch ms; None = never seen.
    pub last_seen: Option<i64>,
    pub last_activity: Option<String>,
    pub framework: Option<String>,
}

/// POST /api/agents/register — upsert by name, mark idle + seen.
/// Answers (id, name) for the wire's `{agent: {id, name}, registered: true}`.
pub async fn register_agent(
    pg: &PgPool,
    name: &str,
    role: Option<&str>,
    framework: Option<&str>,
    capabilities: &serde_json::Value,
) -> Result<(String, String), sqlx::Error> {
    let rec = sqlx::query_as::<_, (String, String)>(
        "insert into fleet_agents (name, role, framework, capabilities, status, last_seen, last_activity) \
         values ($1, $2, $3, $4, 'idle', now(), 'Registered') \
         on conflict (name) do update set \
           role = excluded.role, \
           framework = excluded.framework, \
           capabilities = excluded.capabilities, \
           status = 'idle', \
           last_seen = now(), \
           last_activity = 'Registered', \
           updated_at = now() \
         returning id::text, name",
    )
    .bind(name)
    .bind(role.unwrap_or("agent"))
    .bind(framework)
    .bind(capabilities)
    .fetch_one(pg)
    .await?;
    Ok((rec.0, rec.1))
}

/// Heartbeat by registry id — refresh last_seen; lift offline → idle.
/// Answers the agent's name (for assigned-work lookup), or None when the id
/// is unknown.
pub async fn heartbeat_agent(
    pg: &PgPool,
    id: &str,
    activity: Option<&str>,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "update fleet_agents set \
           last_seen = now(), \
           status = case when status = 'offline' then 'idle' else status end, \
           last_activity = coalesce($2, last_activity), \
           updated_at = now() \
         where id = $1::uuid \
         returning name",
    )
    .bind(id)
    .bind(activity)
    .fetch_optional(pg)
    .await?;
    Ok(row.and_then(|(name,)| name))
}

/// Seed fleet names (from the fleet list) so agents appear before they
/// heartbeat. Insert-if-absent, one statement per name — the roster is
/// small.
pub async fn seed_fleet_names(pg: &PgPool, names: &[String]) -> Result<(), sqlx::Error> {
    for name in names {
        sqlx::query("insert into fleet_agents (name) values ($1) on conflict (name) do nothing")
            .bind(name)
            .execute(pg)
            .await?;
    }
    Ok(())
}

/// Registry keyed by name, with status derived from heartbeat recency:
/// a stale heartbeat reads offline whatever the stored status says; a fresh
/// one lifts offline → idle. `now` is epoch ms (the house rule — callers
/// pass the clock).
pub async fn registry_by_name(
    pg: &PgPool,
    now: i64,
) -> Result<std::collections::HashMap<String, RegistryAgent>, sqlx::Error> {
    let rows: Vec<(
        String,
        String,
        String,
        String,
        Option<i64>,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        "select id::text, name, role, status, \
           (extract(epoch from last_seen) * 1000)::bigint, last_activity, framework \
         from fleet_agents",
    )
    .fetch_all(pg)
    .await?;
    let mut map = std::collections::HashMap::new();
    for (id, name, role, status, last_seen, last_activity, framework) in rows {
        let stored = AgentStatus::parse(&status);
        let fresh = last_seen.is_some_and(|seen| now - seen < FRESH_MS);
        let status = if fresh {
            if stored == AgentStatus::Offline {
                AgentStatus::Idle
            } else {
                stored
            }
        } else {
            AgentStatus::Offline
        };
        map.insert(
            name.clone(),
            RegistryAgent {
                id,
                name,
                role,
                status,
                last_seen,
                last_activity,
                framework,
            },
        );
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_derivation_matches_the_heartbeat_rule() {
        // parse covers the stored vocabulary; anything unknown is offline.
        assert_eq!(AgentStatus::parse("idle"), AgentStatus::Idle);
        assert_eq!(AgentStatus::parse("weird"), AgentStatus::Offline);
        assert_eq!(AgentStatus::Idle.wire(), "idle");
    }
}
