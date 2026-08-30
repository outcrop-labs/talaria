// renderFleet — regenerate everything the managed fleet runs on from the
// database: each managed agent's config.yaml + SOUL.md + git credential helper
// + gitconfig + secrets.env, the shared skills root, and the one compose file
// + manifest that describe the whole fleet. Idempotent, re-run on any change;
// Hermes re-reads config on mtime, so a render lands in running agents without
// a restart. Port of ui/src/server/fleet-render.ts.
//
// THIS MODULE carries the DATA PLANE first: the managed-agent roster, the
// stable loopback port assignment, and the chassis parse. The env/key writers,
// then the render loop and the compose/manifest emit, land in the same arc
// (they drag agent-auth, workbench, secrets, mcp-registry, guardrails, org).
//
// THE CHASSIS PARSE carries one documented YAML resolution divergence. TS
// parses chassis.yml in YAML 1.1 — `mode: 0400` reads as OCTAL 256 — then
// emits compose in 1.2, where 256 lands as the decimal literal `256`. This
// parse (serde_yaml_ng, 1.2 resolution) keeps `0400` a STRING; the emitter
// writes those bytes back; docker's own 1.1 reader re-reads them as octal
// 256 — the same mode TS produces, via a different byte spelling. What this
// does NOT carry is the 1.1 bool set (`on`/`off`/`yes`/`no` stay strings
// here, became booleans in TS): a chassis that spelled a compose boolean as
// `on` would render differently, and no compose idiom does.

use serde::Deserialize;
use serde_json::{Map, Value};
use sqlx::PgPool;
use std::collections::HashMap;

use crate::fleet_layout::{self, GATEWAY_PORT_BASE};

/// The def columns the render loop reads — the agent_defs row for
/// MANAGED+ENABLED agents (the current version rides alongside in
/// [`RenderTarget`]).
#[derive(Debug, Clone)]
pub struct RenderDef {
    pub id: String,
    pub slug: String,
    pub department: String,
    /// The persona id (`<slug>-<department>`) — what grants, manifests, and
    /// gateway headers address.
    pub model: String,
    pub display_name: String,
    pub role: Option<String>,
    pub source: String,
    pub active_slot: Option<String>,
    pub workbench: Option<String>,
    pub workbench_profile: Option<String>,
    pub workbench_harness: Option<String>,
}

/// The version's render-facing columns (the full version shape lives in
/// agent_defs; the loop reads soul + config).
#[derive(Debug, Clone)]
pub struct RenderVersion {
    pub version: i32,
    pub soul: String,
    pub config: Value,
}

#[derive(Debug, Clone)]
pub struct RenderTarget {
    pub def: RenderDef,
    pub version: RenderVersion,
}

/// The managed_agents row shape, spelled once.
type TargetTuple = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i32,
    String,
    Value,
);

fn target_of(r: TargetTuple) -> RenderTarget {
    RenderTarget {
        def: RenderDef {
            id: r.0,
            slug: r.1,
            department: r.2,
            model: r.3,
            display_name: r.4,
            role: r.5,
            source: r.6,
            active_slot: r.7,
            workbench: r.8,
            workbench_profile: r.9,
            workbench_harness: r.10,
        },
        version: RenderVersion {
            version: r.11,
            soul: r.12,
            config: r.13,
        },
    }
}

/// Every managed+enabled agent with its current version, slug-ordered — the
/// render's whole world. Agents without a current version are absent (the
/// join drops them): a def with no version cannot be rendered.
pub async fn managed_agents(pg: &PgPool) -> Result<Vec<RenderTarget>, sqlx::Error> {
    // Literal SQL: one auditable statement, columns spelled once.
    let rows: Vec<TargetTuple> = sqlx::query_as(
        "select d.id::text, d.slug, d.department, d.model, d.display_name, d.role, d.source, \
         d.active_slot, d.workbench, d.workbench_profile, d.workbench_harness, \
         v.version, v.soul, v.config \
         from agent_defs d \
         join agent_versions v on v.agent_id = d.id and v.version = d.current_version \
         where d.managed and d.enabled \
         order by d.slug",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(target_of).collect())
}

/// Assign each managed agent a stable loopback port (persisted, never reused),
/// so the rendered compose and the manifest agree across membership changes.
/// New ports continue past the highest ever assigned — including ports held by
/// agents that no longer render (disabled/unmanaged), which is why the
/// existing-port scan is NOT scoped to the roster.
pub async fn ensure_gateway_ports(
    pg: &PgPool,
    slugs: &[String],
) -> Result<HashMap<String, i64>, sqlx::Error> {
    let existing: Vec<(String, i64)> = sqlx::query_as(
        "select slug, gateway_port::int8 from agent_defs where gateway_port is not null",
    )
    .fetch_all(pg)
    .await?;
    let mut map: HashMap<String, i64> = existing.into_iter().collect();
    let mut next = map
        .values()
        .max()
        .map(|m| m + 1)
        .unwrap_or(GATEWAY_PORT_BASE);
    for slug in slugs {
        if map.contains_key(slug) {
            continue;
        }
        let port = next;
        next += 1;
        sqlx::query("update agent_defs set gateway_port = $1::int4 where slug = $2")
            .bind(port as i32)
            .bind(slug)
            .execute(pg)
            .await?;
        map.insert(slug.clone(), port);
    }
    Ok(map)
}

/// The next unclaimed loopback port for an incoming slot.
pub async fn next_free_port(pg: &PgPool) -> Result<i64, sqlx::Error> {
    let row: (i64,) =
        sqlx::query_as("select coalesce(max(gateway_port), $1)::int8 as m from agent_defs")
            .bind((GATEWAY_PORT_BASE - 1) as i32)
            .fetch_one(pg)
            .await?;
    Ok(row.0.max(GATEWAY_PORT_BASE - 1) + 1)
}

/// Per-agent additions beyond the uniform chassis, keyed by slug.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ChassisExtras {
    #[serde(default)]
    pub environment: Option<Map<String, Value>>,
    #[serde(default)]
    pub volumes: Option<Vec<String>>,
    #[serde(default)]
    pub secrets: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NetworkDef {
    pub name: String,
}

/// The chassis every agent renders from: one service block + per-slug extras.
/// Talaria-owned (extracted once at cutover from the legacy stack). Everything
/// but the service block is optional.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Chassis {
    /// The uniform service block — cloned per agent, then tailored.
    pub service: Value,
    /// Per-agent additions beyond the uniform chassis, keyed by slug.
    #[serde(default)]
    pub extras: HashMap<String, ChassisExtras>,
    /// Shared named-volume definitions (workspaces, repos, kanban).
    #[serde(default)]
    pub volumes: Map<String, Value>,
    /// Secret definitions the extras may reference.
    #[serde(default)]
    pub secrets: Map<String, Value>,
    /// External docker network the fleet joins (fresh installs use their own;
    /// this machine's legacy default is ai_default).
    #[serde(default)]
    pub network: Option<NetworkDef>,
}

/// Parse the chassis YAML into the render's shapes. Pure — the file read (and
/// its two operator-facing errors) lives in [`read_chassis`]. Merge keys
/// (`<<:`) resolve here, matching TS's `parseYaml(text, { merge: true })`.
pub fn parse_chassis(text: &str) -> Result<Chassis, String> {
    let mut yaml: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(text).map_err(|e| format!("chassis.yml is not YAML: {e}"))?;
    yaml.apply_merge()
        .map_err(|e| format!("chassis.yml has an unusable merge key: {e}"))?;
    let json: Value = serde_json::to_value(yaml)
        .map_err(|e| format!("chassis.yml carries a shape JSON cannot hold: {e}"))?;
    let Some(obj) = json.as_object() else {
        return Err("chassis.yml is not a mapping".into());
    };
    if obj.get("service").map(Value::is_object) != Some(true) {
        return Err("chassis.yml has no \"service\" block".into());
    }
    serde_json::from_value(json).map_err(|e| format!("chassis.yml has an unreadable block: {e}"))
}

/// Read + parse the fleet chassis. The two failures are the operator's to fix:
/// a missing file and a file with no service block both stop the render (there
/// is no default chassis — the harness cannot render agents without one).
pub async fn read_chassis() -> Result<Chassis, String> {
    let path = fleet_layout::chassis_file();
    let text = tokio::fs::read_to_string(&path).await.map_err(|_| {
        format!(
            "fleet chassis missing at {} — the harness cannot render agents without it",
            path.display()
        )
    })?;
    parse_chassis(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CHASSIS: &str = "\
service:
  image: hermes:latest
  environment:
    MODE: 0400
  volumes:
    - shared-hooks:/opt/hooks:ro
extras:
  analyst:
    environment:
      EXTRA_TOOL: on
    volumes:
      - workspace:/opt/ws
    secrets:
      - gh
volumes:
  shared-hooks: {}
secrets:
  gh:
    file: ./gh.txt
network:
  name: talaria
";

    #[test]
    fn the_chassis_parses_with_extras_volumes_secrets_and_network() {
        let c = parse_chassis(CHASSIS).unwrap();
        assert_eq!(c.service["image"], json!("hermes:latest"));
        assert_eq!(
            c.service["environment"]["MODE"],
            json!("0400"),
            "1.2 resolution keeps the octal-looking mode a string; see the module header"
        );
        let ex = c.extras.get("analyst").unwrap();
        assert_eq!(ex.environment.as_ref().unwrap()["EXTRA_TOOL"], json!("on"));
        assert_eq!(
            ex.volumes.as_deref().unwrap(),
            &["workspace:/opt/ws".to_string()]
        );
        assert_eq!(ex.secrets.as_ref().unwrap()[0], json!("gh"));
        assert!(c.volumes.contains_key("shared-hooks"));
        assert!(c.secrets.contains_key("gh"));
        assert_eq!(c.network.as_ref().unwrap().name, "talaria");
    }

    #[test]
    fn merge_keys_resolve_like_ts_merge_true() {
        let merged = "\
base: &base
  image: hermes:latest
  environment:
    A: '1'
service:
  <<: *base
  environment:
    B: '2'
";
        let c = parse_chassis(merged).unwrap();
        assert_eq!(
            c.service["image"],
            json!("hermes:latest"),
            "inherited via <<"
        );
        // serde's apply_merge replaces the key wholesale (last wins), same as
        // the yaml package's merge resolution for a whole-key override.
        assert_eq!(c.service["environment"]["B"], json!("2"));
    }

    #[test]
    fn a_chassis_without_a_service_block_is_the_operator_error() {
        assert_eq!(
            parse_chassis("volumes:\n  x: {}\n").unwrap_err(),
            "chassis.yml has no \"service\" block"
        );
        assert!(parse_chassis("not: [a, mapping").is_err());
        assert!(parse_chassis("- just\n- a\n- list\n").is_err());
    }

    #[test]
    fn an_empty_chassis_still_parses_to_defaults() {
        let c = parse_chassis("service:\n  image: x\n").unwrap();
        assert!(c.extras.is_empty());
        assert!(c.network.is_none());
    }
}
