// Sealing the credential literals already stored in agent configs.
//
// Since #579 a config that arrives from outside (federation) is sealed before
// it is stored: each inline credential becomes a per-agent secret
// `CONFIG_<PATH>` and the config keeps a `${CONFIG_<PATH>}` handle
// (talaria-agent-secrets' `plan_config_seal`). Configs stored before that
// still hold the literals — in `agent_versions.config.raw`, in EVERY version
// row, and in the rows every later edit, cascade, MCP change or revert copied
// from them. Any agent reader can fetch them back (`GET …/versions`,
// `GET /api/history?kind=config`), and a revert re-publishes them.
//
// [`seal_stored_configs`] is the admin's one-shot that redacts them, with a
// dry run that only reports. Not a boot migration, because sealing a running
// agent means recreating its container, which an operator triggers.
//
// THE ORDER is what keeps a running agent working throughout. config.yaml
// reaches a running container in place (Hermes reloads it at once), but the
// per-agent secrets arrive only through the env_file a container is CREATED
// with, and Hermes keeps an unresolved `${…}` as its literal text. So a handle
// must never reach a container that lacks its variable:
//
//   1. seal the current version's literals as the agent's secrets (rows only;
//      no config changes yet);
//   2. if the agent is running, roll it — the render's config.yaml is still
//      the literal one, byte for byte, and the new container starts with the
//      `CONFIG_*` env. If its container exists but is stopped, remove it:
//      `compose restart` (the roster's restart, a cascade's) would start that
//      container again with the env it was created with, which lacks them,
//      while `up` (Start, reconcile) creates a fresh one from the render;
//   3. rewrite every version row to handles, in place;
//   4. render once more, so config.yaml takes the handles, which now resolve.
//
// An agent with no container at all needs neither: its next start creates
// one from the render, which holds both. A container whose state docker
// cannot report skips the agent; nothing of it is touched.
//
// Each agent audits as soon as its outcome is known, names and paths only:
// `agent.config_sealed`, or `agent.config_seal_incomplete` when its secrets
// were stored but its configs were not rewritten (the roll failed, an edit
// landed meanwhile). The route runs this detached, so a caller that hangs up
// does not stop it halfway.
//
// History is rewritten in place rather than appended to: an appended version
// would leave the old literals servable and revertible. The handle resolves
// to the same value, so this is a redaction, not a change. An older version
// that held a DIFFERENT value at the same path gets the same handle, which
// resolves to the current value (#579's documented semantics: a revert gets
// the current value behind a handle); the report names those paths. A path
// only older versions held is rewritten to a handle with no secret behind it
// — never minted from the old value, which would bring a rotated or removed
// credential back into the agent's env — and a revert to such a version
// leaves the setting out (fleet_defs_id_versions).

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::{BTreeMap, HashSet};

use talaria_agent_secrets::{
    ConfigLiteral, held_config_secrets, plan_config_seal_dropping_undeliverable,
    seal_config_literals,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_secretbox::SecretBox;

/// What the admin asked for. Owned, so the route can run it detached.
#[derive(Debug, Clone)]
pub struct SealRequest {
    /// Report only: nothing is written, nothing rolls.
    pub dry_run: bool,
    /// These agents (the ones a dry run listed), or every agent.
    pub agent_ids: Option<Vec<String>>,
    /// Take a literal no per-agent secret can carry (multi-line, over 8 KB)
    /// out of the config, instead of leaving that agent as it is.
    pub drop_undeliverable: bool,
    /// Who the sealed secrets are stamped as updated by.
    pub actor: String,
    /// Who each agent's audit row names.
    pub audit_as: String,
}

/// One agent's part of the report. Names and paths only, never a value.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSeal {
    pub agent_id: String,
    pub slug: String,
    pub display_name: String,
    /// `clean` (nothing stored as a literal), `planned` (the dry run's
    /// verdict), `sealed`, or `skipped` (see `reason`).
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The versions whose stored config changes.
    pub versions: Vec<i32>,
    /// The per-agent secrets sealed from the current version.
    pub sealed: Vec<String>,
    /// Paths where an older version held a different value than the current
    /// one: they now name the current value. `path (v1, v2)`.
    pub changed: Vec<String>,
    /// Paths only older versions held: rewritten to a handle with no secret
    /// behind it, so a revert to them leaves the setting out. `path (v1)`.
    pub history_only: Vec<String>,
    /// Literals Talaria supplies itself (a model spec's `api_key`, an
    /// authored MCP server's credential, the API server's key): removed.
    pub dropped: Vec<String>,
    /// Literals no per-agent secret can carry: removed when the request asked
    /// for it, otherwise the reason this agent is skipped.
    pub undeliverable: Vec<String>,
    /// Whether applying recreates its container (it is running, and gains
    /// secrets).
    pub roll: bool,
    pub rolled: bool,
    /// Whether applying removes its container (it exists, stopped, and the
    /// agent gains secrets), so its next start creates one that holds them.
    pub remove_stopped: bool,
    pub removed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SealReport {
    pub dry_run: bool,
    pub agents: Vec<AgentSeal>,
    /// The closing render's warnings, or its failure.
    pub warnings: Vec<String>,
}

/// One stored version, as read.
#[derive(Debug, Clone)]
pub struct StoredVersion {
    pub id: String,
    pub version: i32,
    pub config: Value,
}

/// What sealing one agent's history comes to, worked out before anything is
/// written. Pure.
#[derive(Debug, Clone, Default)]
pub struct DefPlan {
    /// (version row id, version, the config as read, the config to store).
    pub rewrites: Vec<(String, i32, Value, Value)>,
    /// The current version's literals: the secrets to seal.
    pub sealed: Vec<ConfigLiteral>,
    pub changed: Vec<String>,
    pub history_only: Vec<String>,
    pub dropped: Vec<String>,
    pub undeliverable: Vec<String>,
    /// A literal that cannot be sealed, when the request did not ask to drop
    /// it: nothing of this agent is touched.
    pub refused: Option<String>,
}

fn by_path(map: BTreeMap<String, Vec<i32>>) -> Vec<String> {
    map.into_iter()
        .map(|(path, mut versions)| {
            versions.sort_unstable();
            versions.dedup();
            let vs: Vec<String> = versions.iter().map(|v| format!("v{v}")).collect();
            format!("{path} ({})", vs.join(", "))
        })
        .collect()
}

/// Plan one agent: every version, the current one's literals as the secrets
/// (`held` is the `CONFIG_*` names the agent already has).
pub fn plan_def(
    versions: &[StoredVersion],
    current: i32,
    held: &HashSet<String>,
    drop_undeliverable: bool,
) -> DefPlan {
    let mut out = DefPlan::default();
    let current = versions
        .iter()
        .find(|v| v.version == current)
        .or_else(|| versions.iter().max_by_key(|v| v.version))
        .map(|v| v.version);
    // Always planned with undeliverable literals taken out, so the report can
    // name every one of them; without `drop_undeliverable` any of them
    // refuses the agent below.
    let mut plans = Vec::with_capacity(versions.len());
    for v in versions {
        match plan_config_seal_dropping_undeliverable(&v.config) {
            Ok(p) => plans.push((v, p)),
            Err(why) => {
                return DefPlan {
                    refused: Some(format!("v{}: {why}", v.version)),
                    ..DefPlan::default()
                };
            }
        }
    }
    if let Some((_, p)) = plans.iter().find(|(v, _)| Some(v.version) == current) {
        out.sealed = p.sealed.clone();
    }
    let mut changed: BTreeMap<String, Vec<i32>> = BTreeMap::new();
    let mut history_only: BTreeMap<String, Vec<i32>> = BTreeMap::new();
    let mut dropped: BTreeMap<String, Vec<i32>> = BTreeMap::new();
    let mut undeliverable: BTreeMap<String, Vec<i32>> = BTreeMap::new();
    for (v, p) in &plans {
        for path in &p.dropped {
            dropped.entry(path.clone()).or_default().push(v.version);
        }
        for path in &p.undeliverable {
            undeliverable
                .entry(path.clone())
                .or_default()
                .push(v.version);
        }
        if Some(v.version) != current {
            for lit in &p.sealed {
                match out.sealed.iter().find(|c| c.name == lit.name) {
                    Some(cur) if cur.value != lit.value => {
                        changed.entry(lit.path.clone()).or_default().push(v.version);
                    }
                    Some(_) => {}
                    // The agent already holds this name: the handle resolves
                    // to what it holds, which nothing here opens to compare.
                    None if held.contains(&lit.name) => {}
                    None => {
                        history_only
                            .entry(lit.path.clone())
                            .or_default()
                            .push(v.version);
                    }
                }
            }
        }
        if p.config != v.config {
            out.rewrites
                .push((v.id.clone(), v.version, v.config.clone(), p.config.clone()));
        }
    }
    out.rewrites.sort_by_key(|r| r.1);
    out.changed = by_path(changed);
    out.history_only = by_path(history_only);
    out.dropped = by_path(dropped);
    out.undeliverable = by_path(undeliverable);
    if !drop_undeliverable && !out.undeliverable.is_empty() {
        return DefPlan {
            refused: Some(format!(
                "{} {} a literal credential no per-agent secret can carry (multi-line, or over 8 KB)",
                out.undeliverable.join(", "),
                if out.undeliverable.len() == 1 {
                    "holds"
                } else {
                    "hold"
                }
            )),
            undeliverable: out.undeliverable,
            ..DefPlan::default()
        };
    }
    out
}

type DefRow = (String, String, String, String, bool, bool, i32);

/// Seal every stored literal (or report what that would do). See the
/// module header for the order and why.
pub async fn seal_stored_configs(
    pg: &PgPool,
    sb: &SecretBox,
    req: &SealRequest,
) -> Result<SealReport, String> {
    let defs: Vec<DefRow> = sqlx::query_as(
        "select id::text, slug, department, display_name, managed, enabled, current_version \
         from agent_defs where ($1::text[] is null or id::text = any($1)) order by slug",
    )
    .bind(&req.agent_ids)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("agent defs read failed: {e}"))?;
    let mut report = SealReport {
        dry_run: req.dry_run,
        agents: Vec::with_capacity(defs.len()),
        warnings: Vec::new(),
    };
    // Only a managed agent's config is rendered; an unmanaged one's history
    // is redacted and that is all.
    let mut rerender = false;
    for (id, slug, department, display_name, managed, enabled, current) in defs {
        let rows: Vec<(String, i32, Value)> = sqlx::query_as(
            "select id::text, version, config from agent_versions \
             where agent_id = $1::uuid order by version",
        )
        .bind(&id)
        .fetch_all(pg)
        .await
        .map_err(|e| format!("{slug}: versions read failed: {e}"))?;
        let versions: Vec<StoredVersion> = rows
            .into_iter()
            .map(|(id, version, config)| StoredVersion {
                id,
                version,
                config,
            })
            .collect();
        let held: HashSet<String> = held_config_secrets(pg, &id)
            .await
            .map_err(|e| format!("{slug}: agent secrets read failed: {e}"))?
            .into_iter()
            .collect();
        let plan = plan_def(&versions, current, &held, req.drop_undeliverable);
        let mut agent = AgentSeal {
            agent_id: id.clone(),
            slug: slug.clone(),
            display_name,
            status: "clean",
            versions: plan.rewrites.iter().map(|r| r.1).collect(),
            sealed: plan.sealed.iter().map(|l| l.name.clone()).collect(),
            changed: plan.changed.clone(),
            history_only: plan.history_only.clone(),
            dropped: plan.dropped.clone(),
            undeliverable: plan.undeliverable.clone(),
            ..AgentSeal::default()
        };
        if let Some(why) = &plan.refused {
            agent.status = "skipped";
            agent.reason = Some(if plan.undeliverable.is_empty() {
                format!("{why}. Nothing of this agent was changed")
            } else {
                format!(
                    "{why}. Nothing of this agent was changed; ask again with dropUndeliverable to take that setting out"
                )
            });
            report.agents.push(agent);
            continue;
        }
        if plan.rewrites.is_empty() {
            report.agents.push(agent);
            continue;
        }
        // A managed agent that gains secrets needs a container that holds
        // them before any handle reaches its config.yaml: a running one is
        // recreated, a stopped one removed (see the module header).
        let container = if managed && !plan.sealed.is_empty() {
            let name = talaria_fleet_docker::slot_container(
                &department,
                talaria_fleet_docker::active_slot(pg, &department).await,
            );
            match talaria_fleet_docker::container_state(&name).await {
                Ok(Some(true)) => agent.roll = enabled,
                Ok(Some(false)) => agent.remove_stopped = true,
                Ok(None) => {}
                Err(e) => {
                    agent.status = "skipped";
                    agent.reason = Some(format!(
                        "its container's state could not be read ({e}). Nothing of this agent was changed; ask again"
                    ));
                    report.agents.push(agent);
                    continue;
                }
            }
            Some(name)
        } else {
            None
        };
        if req.dry_run {
            agent.status = "planned";
            report.agents.push(agent);
            continue;
        }

        // 1. The secrets, before anything names them.
        if let Err(e) = seal_config_literals(pg, sb, &id, &plan.sealed, Some(&req.actor)).await {
            agent.status = "skipped";
            agent.reason = Some(format!("sealing failed: {e}. Nothing was changed"));
            report.agents.push(agent);
            continue;
        }
        // 2. A container that holds them: the roll, or the stopped one gone.
        let step2 = if agent.roll {
            match talaria_fleet_reconcile::roll_agent(pg, sb, &department).await {
                Ok(None) => {
                    agent.rolled = true;
                    Ok(())
                }
                Ok(Some(soft)) => Err(format!("the roll did not finish ({soft})")),
                Err(e) => Err(format!("the roll did not finish ({e})")),
            }
        } else if let (true, Some(name)) = (agent.remove_stopped, &container) {
            match talaria_fleet_docker::remove_stopped_container(name).await {
                Ok(()) => {
                    agent.removed = true;
                    Ok(())
                }
                Err(e) => Err(format!("its stopped container could not be removed ({e})")),
            }
        } else {
            Ok(())
        };
        if let Err(why) = step2 {
            agent.status = "skipped";
            agent.reason = Some(format!(
                "{why}. Its secrets are stored, its configs are not rewritten; ask again"
            ));
            audit(pg, req, &agent).await;
            report.agents.push(agent);
            continue;
        }
        // 3. Every version, in place — only if nothing moved since the read.
        match rewrite(pg, &id, versions.len(), &plan.rewrites).await {
            Ok(true) => {
                agent.status = "sealed";
                rerender |= managed;
            }
            Ok(false) => {
                agent.status = "skipped";
                agent.reason = Some(
                    "its versions changed while this ran (an edit landed). Its secrets are stored, its configs are not rewritten; ask again".into(),
                );
            }
            Err(e) => {
                agent.status = "skipped";
                agent.reason = Some(format!(
                    "rewriting its versions failed: {e}. Its secrets are stored, its configs are as they were"
                ));
            }
        }
        audit(pg, req, &agent).await;
        report.agents.push(agent);
    }
    // 4. One render, so the handles reach every rendered config.yaml.
    if rerender {
        match talaria_fleet_render::render_fleet(pg, sb).await {
            Ok(r) => report.warnings.extend(r.warnings),
            Err(e) => report.warnings.push(format!(
                "the closing render failed ({e}): the stored configs are sealed, the rendered files catch up on the next render"
            )),
        }
    }
    Ok(report)
}

/// One applied agent's audit row, names and paths only. A skipped agent
/// whose secrets were stored is `agent.config_seal_incomplete`: secrets were
/// written, and nothing else says so. One that stored nothing has nothing to
/// audit.
async fn audit(pg: &PgPool, req: &SealRequest, agent: &AgentSeal) {
    let (action, after) = match agent.status {
        "sealed" => (
            "agent.config_sealed",
            json!({
                "versions": agent.versions,
                "sealed": agent.sealed,
                "changed": agent.changed,
                "historyOnly": agent.history_only,
                "dropped": agent.dropped,
                "undeliverable": agent.undeliverable,
                "rolled": agent.rolled,
                "removedStoppedContainer": agent.removed,
            }),
        ),
        _ if !agent.sealed.is_empty() => (
            "agent.config_seal_incomplete",
            json!({
                "sealed": agent.sealed,
                "reason": agent.reason,
                "rolled": agent.rolled,
                "removedStoppedContainer": agent.removed,
            }),
        ),
        _ => return,
    };
    log_audit(
        pg,
        AuditEntry {
            actor: &req.audit_as,
            action,
            target_type: "agent",
            target_id: Some(&agent.agent_id),
            target_label: Some(&agent.display_name),
            before: None,
            after: Some(after),
        },
    )
    .await;
}

/// Rewrite the planned rows in one transaction. False (and nothing written)
/// when a row changed since it was read, or a version was added.
async fn rewrite(
    pg: &PgPool,
    agent_id: &str,
    expected_rows: usize,
    rewrites: &[(String, i32, Value, Value)],
) -> Result<bool, sqlx::Error> {
    let mut tx = pg.begin().await?;
    let count: i64 = sqlx::query_scalar(
        "select count(*) from (select 1 from agent_versions where agent_id = $1::uuid for update) v",
    )
    .bind(agent_id)
    .fetch_one(&mut *tx)
    .await?;
    if count != expected_rows as i64 {
        tx.rollback().await?;
        return Ok(false);
    }
    for (row_id, _, read, sealed) in rewrites {
        let hit = sqlx::query(
            "update agent_versions set config = $3::jsonb where id = $1::uuid and config = $2::jsonb",
        )
        .bind(row_id)
        .bind(read)
        .bind(sealed)
        .execute(&mut *tx)
        .await?;
        if hit.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
    }
    // The row locks hold the rows read; they do not stop a version writer
    // (`add_version_if_changed`) inserting the next one. A version that
    // committed while these updates ran copied a config read before them,
    // literal and all: write nothing, and the caller asks again. One that
    // commits after this one cannot be caught here. The next run (its dry
    // run lists it) seals it, as it would a literal pasted in later.
    let recount: i64 =
        sqlx::query_scalar("select count(*) from agent_versions where agent_id = $1::uuid")
            .bind(agent_id)
            .fetch_one(&mut *tx)
            .await?;
    if recount != expected_rows as i64 {
        tx.rollback().await?;
        return Ok(false);
    }
    tx.commit().await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn version(version: i32, raw: Value) -> StoredVersion {
        StoredVersion {
            id: format!("row-{version}"),
            version,
            config: json!({ "main": { "endpoint": "e", "model": "m" }, "raw": raw }),
        }
    }

    #[test]
    fn every_version_is_sealed_and_the_current_value_is_the_one_kept() {
        let versions = vec![
            version(
                1,
                json!({
                    "platforms": { "telegram": { "enabled": true, "token": "A" }, "slack": { "bot_token": "old-slack" } },
                }),
            ),
            version(
                2,
                json!({
                    "platforms": { "telegram": { "enabled": true, "token": "B" } },
                    "model": { "provider": "custom", "model": "m", "api_key": "sk-upstream" },
                }),
            ),
        ];
        let plan = plan_def(&versions, 2, &HashSet::new(), false);
        assert!(plan.refused.is_none());
        // The secret is the CURRENT value.
        assert_eq!(plan.sealed.len(), 1);
        assert_eq!(plan.sealed[0].name, "CONFIG_PLATFORMS_TELEGRAM_TOKEN");
        assert_eq!(plan.sealed[0].value, "B");
        // Both rows are rewritten, each to handles.
        assert_eq!(
            plan.rewrites.iter().map(|r| r.1).collect::<Vec<_>>(),
            vec![1, 2]
        );
        for (_, _, read, sealed) in &plan.rewrites {
            assert_ne!(read, sealed);
            let text = sealed.to_string();
            for literal in ["\"A\"", "\"B\"", "old-slack", "sk-upstream"] {
                assert!(!text.contains(literal), "{literal} left in {text}");
            }
            assert_eq!(
                sealed["raw"]["platforms"]["telegram"]["token"],
                json!("${CONFIG_PLATFORMS_TELEGRAM_TOKEN}")
            );
        }
        assert_eq!(
            plan.changed,
            vec!["platforms.telegram.token (v1)"],
            "v1's different value now names v2's"
        );
        assert_eq!(
            plan.history_only,
            vec!["platforms.slack.bot_token (v1)"],
            "never minted from the old value"
        );
        assert_eq!(plan.dropped, vec!["model.api_key (v2)"]);
    }

    #[test]
    fn a_second_run_plans_nothing() {
        let versions = vec![
            version(1, json!({ "platforms": { "telegram": { "token": "A" } } })),
            version(2, json!({ "platforms": { "telegram": { "token": "B" } } })),
        ];
        let first = plan_def(&versions, 2, &HashSet::new(), false);
        let sealed: Vec<StoredVersion> = first
            .rewrites
            .iter()
            .map(|(id, v, _, config)| StoredVersion {
                id: id.clone(),
                version: *v,
                config: config.clone(),
            })
            .collect();
        let held: HashSet<String> = first.sealed.iter().map(|l| l.name.clone()).collect();
        let again = plan_def(&sealed, 2, &held, false);
        assert!(again.rewrites.is_empty(), "{:?}", again.rewrites);
        assert!(again.sealed.is_empty());
        assert!(again.changed.is_empty() && again.history_only.is_empty());
    }

    #[test]
    fn an_undeliverable_literal_skips_the_agent_unless_asked_to_drop_it() {
        let versions = vec![
            version(
                1,
                json!({ "platforms": { "matrix": { "password": "a\nb" } } }),
            ),
            version(2, json!({ "platforms": { "telegram": { "token": "B" } } })),
        ];
        let refused = plan_def(&versions, 2, &HashSet::new(), false);
        let why = refused.refused.expect("refused");
        assert!(
            why.starts_with("platforms.matrix.password (v1) holds a literal credential"),
            "{why}"
        );
        assert!(refused.rewrites.is_empty() && refused.sealed.is_empty());
        // The report still names it, so the admin sees what dropping takes out.
        assert_eq!(
            refused.undeliverable,
            vec!["platforms.matrix.password (v1)"]
        );

        let dropped = plan_def(&versions, 2, &HashSet::new(), true);
        assert!(dropped.refused.is_none());
        assert_eq!(
            dropped.undeliverable,
            vec!["platforms.matrix.password (v1)"]
        );
        assert_eq!(dropped.rewrites.len(), 2);
        assert_eq!(
            dropped.rewrites[0].3["raw"]["platforms"]["matrix"],
            json!({})
        );
    }

    #[test]
    fn a_name_the_agent_already_holds_is_not_history_only() {
        // v2 was sealed since (a federation after #579); v1 still holds the
        // literal. The handle resolves to what the agent holds.
        let versions = vec![
            version(1, json!({ "platforms": { "telegram": { "token": "A" } } })),
            version(
                2,
                json!({ "platforms": { "telegram": { "token": "${CONFIG_PLATFORMS_TELEGRAM_TOKEN}" } } }),
            ),
        ];
        let held: HashSet<String> = ["CONFIG_PLATFORMS_TELEGRAM_TOKEN".to_string()].into();
        let plan = plan_def(&versions, 2, &held, false);
        assert!(plan.sealed.is_empty(), "nothing new to seal");
        assert!(plan.history_only.is_empty() && plan.changed.is_empty());
        assert_eq!(
            plan.rewrites.iter().map(|r| r.1).collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    fn a_clean_agent_plans_nothing() {
        let versions = vec![version(
            1,
            json!({ "model": { "api_key": "${LLM_API_KEY}" } }),
        )];
        let plan = plan_def(&versions, 1, &HashSet::new(), false);
        assert!(plan.rewrites.is_empty() && plan.sealed.is_empty() && plan.refused.is_none());
    }
}
