// Cascading model/endpoint removal: when models (or a whole endpoint) are
// deleted from the registry while agents still target them, reconfigure those
// agents — ONE new version per agent with every stripped tier (auditable),
// one re-render, then one apply wave. An agent's MAIN model is never cascaded
// away: that requires an explicit reassignment.
//
// THE APPLY WAVE NEVER STARTS AN AGENT SOMEONE STOPPED. It used to be a
// `compose restart` per changed agent, and compose restarts a stopped
// container as readily as a running one — so deleting a model brought back
// every agent a manager had switched off. Now each changed agent APPLIES
// (fleet-reconcile's `apply_after_render`): a running one whose container the
// render moved is rolled at once — the stripped tiers are `model_routes` and
// the fallback chain, which Hermes reads only at start, so it always moves —
// and a stopped one stays stopped, reading the render when it next starts
// (`up` renders, and compose recreates a container whose service moved).

use serde_json::{Map, Value};
use sqlx::PgPool;

use talaria_agent_defs::{NewVersion, add_version_if_changed, list_versions};
use talaria_secretbox::SecretBox;

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub slug: String,
    pub def_id: String,
    pub department: String,
    pub managed: bool,
    pub enabled: bool,
    pub as_main: bool,
    /// Alias names targeting the removed endpoint/models.
    pub aliases: Vec<String>,
    /// Count of fallback entries targeting them.
    pub fallbacks: usize,
}

/// hits(t, endpoint, models) — `models: None` means "any model on the
/// endpoint" (a whole-endpoint removal).
fn hits(t: Option<&Value>, endpoint: &str, models: Option<&[String]>) -> bool {
    let Some(t) = t else {
        return false;
    };
    let (Some(ep), Some(model)) = (
        t.get("endpoint").and_then(Value::as_str),
        t.get("model").and_then(Value::as_str),
    ) else {
        return false;
    };
    ep == endpoint && models.is_none_or(|ms| ms.iter().any(|m| m == model))
}

/// Every ENABLED agent whose CURRENT version targets endpoint (+models).
/// Retired (disabled) agents don't run and don't render — their historical
/// versions must not block deleting an endpoint (model_usage).
pub async fn model_usage(
    pg: &PgPool,
    endpoint: &str,
    models: Option<&[String]>,
) -> Result<Vec<ModelUsage>, sqlx::Error> {
    let rows: Vec<(String, String, String, bool, bool, Value)> = sqlx::query_as(
        "select d.id::text, d.slug, d.department, d.managed, d.enabled, v.config \
         from agent_defs d \
         join agent_versions v on v.agent_id = d.id and v.version = d.current_version \
         where d.enabled",
    )
    .fetch_all(pg)
    .await?;
    let mut out = Vec::new();
    for (id, slug, department, managed, enabled, config) in rows {
        let aliases_of = |key: &str| -> Vec<Value> {
            config
                .get(key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        let as_main = hits(config.get("main"), endpoint, models);
        let aliases: Vec<String> = aliases_of("aliases")
            .iter()
            .filter(|a| hits(Some(a), endpoint, models))
            .filter_map(|a| a.get("name").and_then(Value::as_str).map(String::from))
            .collect();
        let fallbacks = aliases_of("fallbacks")
            .iter()
            .filter(|f| hits(Some(f), endpoint, models))
            .count();
        if as_main || !aliases.is_empty() || fallbacks > 0 {
            out.push(ModelUsage {
                slug,
                def_id: id,
                department,
                managed,
                enabled,
                as_main,
                aliases,
                fallbacks,
            });
        }
    }
    Ok(out)
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CascadeResult {
    pub changed: Vec<String>,
    /// Set when the post-cascade render failed — agent VERSIONS are already
    /// written; re-render from /agents once the cause is fixed. The rolls
    /// after a good render run detached: each one's verdict lands in the roll
    /// registry (the roster's chip) and the log, not here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub render_error: Option<String>,
}

/// Strip alias/fallback targets of endpoint(+models) from every affected agent
/// (one new version each), then re-render once and apply to each changed
/// managed agent — a roll for a running one the render moved, nothing for a
/// stopped one. Callers must have verified no agent uses the targets as MAIN.
pub async fn cascade_removal(
    pg: &PgPool,
    sb: &SecretBox,
    endpoint: &str,
    models: Option<&[String]>,
    actor: &str,
) -> Result<CascadeResult, sqlx::Error> {
    let affected = model_usage(pg, endpoint, models).await?;
    let mut changed: Vec<String> = Vec::new();
    let what = match models {
        Some(ms) => format!("{}/{}", endpoint, ms.join(",")),
        None => endpoint.to_string(),
    };
    for u in &affected {
        let Some(latest) = list_versions(pg, &u.def_id).await?.into_iter().next() else {
            continue;
        };
        let cfg = &latest.config;
        let kept = |key: &str| -> Vec<Value> {
            cfg.get(key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|t| !hits(Some(t), endpoint, models))
                .collect()
        };
        let kept_aliases = kept("aliases");
        let kept_fallbacks = kept("fallbacks");
        // raw.model_aliases loses the stripped alias names; raw
        // .fallback_providers drops by INDEX alongside the parsed fallbacks
        // (the raw array is the source the parse came from).
        let mut raw = match cfg.get("raw") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        let mut raw_aliases = match raw.get("model_aliases") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        for a in cfg
            .get("aliases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if hits(Some(a), endpoint, models)
                && let Some(name) = a.get("name").and_then(Value::as_str)
            {
                raw_aliases.remove(name);
            }
        }
        raw.insert("model_aliases".into(), Value::Object(raw_aliases));
        if let Some(Value::Array(fp)) = raw.get("fallback_providers").cloned().as_ref() {
            let cfg_fallbacks = cfg
                .get("fallbacks")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let kept_fp: Vec<Value> = fp
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    let f = cfg_fallbacks.get(*i);
                    f.is_none_or(|f| !hits(Some(f), endpoint, models))
                })
                .map(|(_, v)| v.clone())
                .collect();
            raw.insert("fallback_providers".into(), Value::Array(kept_fp));
        }
        let mut next = cfg.clone();
        next["aliases"] = Value::Array(kept_aliases);
        next["fallbacks"] = Value::Array(kept_fallbacks);
        next["raw"] = Value::Object(raw);
        let (_, created) = add_version_if_changed(
            pg,
            &u.def_id,
            &NewVersion {
                soul: &latest.soul,
                config: &next,
                note: Some(&format!("removed {what} (deleted in Models)")),
                created_by: Some(actor),
            },
        )
        .await?;
        if created {
            changed.push(u.slug.clone());
        }
    }
    if changed.is_empty() {
        return Ok(CascadeResult {
            changed,
            render_error: None,
        });
    }
    let rendered = match talaria_fleet_create::RENDER_FLEET.get() {
        Some(rf) => rf(pg.clone(), sb.clone()).await,
        None => Err("fleet renderer is not wired".into()),
    };
    match rendered {
        Ok(_) => {
            let departments = apply_targets(&affected, &changed);
            if !departments.is_empty() {
                // Detached: a roll waits for its container to report healthy
                // (minutes, for several agents), and the caller is an admin's
                // save. The versions and the render have landed already.
                let (pg, sb) = (pg.clone(), sb.clone());
                tokio::spawn(async move {
                    let verdicts =
                        talaria_fleet_reconcile::apply_after_render(&pg, &sb, &departments).await;
                    for (department, verdict) in verdicts {
                        let error = match verdict {
                            Ok(None) => continue,
                            Ok(Some(soft)) => soft,
                            Err(thrown) => thrown,
                        };
                        tracing::warn!(
                            "[fleet] cascade: {department} did not take the model removal: {error}"
                        );
                    }
                });
            }
            Ok(CascadeResult {
                changed,
                render_error: None,
            })
        }
        Err(e) => Ok(CascadeResult {
            changed,
            render_error: Some(e),
        }),
    }
}

/// The departments a cascade applies to: each changed agent Talaria runs.
/// Running or stopped is not decided here — the apply asks docker, and
/// leaves a stopped agent stopped.
fn apply_targets(affected: &[ModelUsage], changed: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for u in affected {
        if u.managed && u.enabled && changed.contains(&u.slug) && !out.contains(&u.department) {
            out.push(u.department.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(slug: &str, department: &str, managed: bool) -> ModelUsage {
        ModelUsage {
            slug: slug.into(),
            def_id: format!("id-{slug}"),
            department: department.into(),
            managed,
            enabled: true,
            as_main: false,
            aliases: vec!["fast".into()],
            fallbacks: 0,
        }
    }

    #[test]
    fn the_apply_wave_names_each_changed_managed_agent_once() {
        let affected = vec![
            usage("ada", "ops", true),
            usage("bob", "eng", true),
            // An external agent: Talaria renders it, but runs no container.
            usage("cy", "ext", false),
            // Unchanged (its version already said this): nothing to apply.
            usage("dee", "fin", true),
        ];
        let changed = vec!["ada".to_string(), "bob".to_string(), "cy".to_string()];
        assert_eq!(apply_targets(&affected, &changed), vec!["ops", "eng"]);
        assert!(apply_targets(&affected, &[]).is_empty());
    }
}
