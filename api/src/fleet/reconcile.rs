// Fleet reconciliation — port of ui/src/server/fleet-reconcile.ts's rollAgent
// and reconcileFleet. A ROLL is the blue/green cutover: render the incoming
// slot alongside the active one, bring it up, wait for real health, flip the
// manifest, drain, retire the old container. In-flight replies never hit a
// dead container, and an unhealthy replacement never takes over — the old
// container keeps serving and the roll reports why.

use sqlx::PgPool;

use std::collections::HashSet;

use crate::fleet::docker::Slot;
use crate::fleet::render::{RollOverlay, next_free_port, render_fleet};
use crate::secretbox::SecretBox;

/// How long the old container keeps serving after cutover so in-flight
/// replies drain (fleet-reconcile.ts ROLL_DRAIN_MS).
fn roll_drain_ms() -> u64 {
    std::env::var("TALARIA_ROLL_DRAIN_SECONDS")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(45.0)
        .max(0.0) as u64
        * 1000
}

/// The roll verdict. `Err` is a thrown step (the caller decides what that
/// means); `Ok(Some(error))` is TS's `{ ok: false, error }` — a deliberate
/// soft failure with a sentence for the UI.
///
/// Everything a running agent's config bakes in at process start — MCP
/// servers above all — becomes live only through this.
pub async fn roll_agent(
    pg: &PgPool,
    sb: &SecretBox,
    department: &str,
) -> Result<Option<String>, String> {
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "select slug, display_name, active_slot from agent_defs \
         where department = $1 and managed and enabled",
    )
    .bind(department)
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    let Some((slug, display_name, active)) = rows.first().cloned() else {
        return Ok(Some(format!(
            "no managed agent in department \"{department}\""
        )));
    };
    let old_slot = if active.as_deref() == Some("b") {
        Slot::B
    } else {
        Slot::A
    };
    let new_slot = if old_slot == Slot::A {
        Slot::B
    } else {
        Slot::A
    };
    let new_port = next_free_port(pg).await.map_err(|e| e.to_string())?;

    // 1. Overlay render: both slots in the compose file; manifest still old.
    render_fleet(
        pg,
        sb,
        Some(RollOverlay {
            slug: &slug,
            slot: new_slot,
            port: new_port,
        }),
    )
    .await?;
    // 2. Bring the incoming slot up and wait for real health.
    crate::fleet::docker::fleet_up_slot(pg, department, new_slot).await?;
    if !crate::fleet::docker::wait_healthy_slot(department, new_slot, 120_000).await {
        let _ = crate::fleet::docker::remove_container_by_name(
            &crate::fleet::docker::slot_container(department, new_slot),
        )
        .await;
        render_fleet(pg, sb, None).await?; // back to steady state; the old container never blinked
        return Ok(Some(format!(
            "{display_name}: replacement never became healthy — kept the old container"
        )));
    }
    // Toolkit-first by default: strip the image's bundled note-tool skills the
    // moment the newcomer is healthy (Talaria-managed skills are untouched).
    let _ = crate::fleet::docker::prune_bundled_skills(department, new_slot).await;
    // 3. Cutover: incoming slot becomes active (new port), manifest re-renders.
    sqlx::query("update agent_defs set active_slot = $2, gateway_port = $3 where slug = $1")
        .bind(&slug)
        .bind(if new_slot == Slot::B { "b" } else { "a" })
        .bind(new_port)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
    render_fleet(pg, sb, None).await?;
    // 4. Drain in-flight replies on the old container, then retire it.
    tokio::time::sleep(std::time::Duration::from_millis(roll_drain_ms())).await;
    let _ = crate::fleet::docker::remove_container_by_name(&crate::fleet::docker::slot_container(
        department, old_slot,
    ))
    .await;
    Ok(None)
}

/// Propagate an identity-level change (e.g. the org profile) to the live
/// fleet: re-render every managed soul, then ROLL running agents one at a
/// time so nobody's conversation ever hits a dead container. Agents someone
/// deliberately stopped stay stopped (they read the new render on next
/// start).
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileResult {
    pub rendered: usize,
    pub started: Vec<String>,
    pub already_running: Vec<String>,
    pub warnings: Vec<String>,
}

/// Bring the running fleet to the desired state in one shot: re-render every
/// managed agent's config, then start any enabled managed agent whose
/// container isn't running. Reboot survival is already handled by `restart:
/// unless-stopped` on the generated services; this covers drift (an agent
/// enabled/created while Talaria was down, a stopped container, a manifest
/// change) — reconcileFleet.
pub async fn reconcile_fleet(pg: &PgPool, sb: &SecretBox) -> Result<ReconcileResult, String> {
    let render = render_fleet(pg, sb, None).await?;
    let managed: Vec<(String, String)> = sqlx::query_as(
        "select department, display_name from agent_defs where managed and enabled order by slug",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    let states = if managed.is_empty() {
        Vec::new()
    } else {
        crate::fleet::docker::container_status(
            &managed.iter().map(|m| m.0.clone()).collect::<Vec<_>>(),
        )
        .await
        .unwrap_or_default()
    };
    let running = |dept: &str| {
        states
            .iter()
            .find(|s| s.department == dept)
            .and_then(|s| s.managed.as_ref())
            .is_some_and(|m| m.state == "running")
    };
    let mut started = Vec::new();
    let mut already_running = Vec::new();
    let mut warnings = render.warnings.clone();
    for (department, display_name) in &managed {
        if running(department) {
            already_running.push(display_name.clone());
            continue;
        }
        match crate::fleet::docker::fleet_up(pg, department).await {
            Ok(_) => {
                started.push(display_name.clone());
                // New containers get the bundled note-tool skills stripped once
                // healthy — toolkit-first is the default from first boot.
                // Detached: reconcile must not block on health.
                let dept = department.clone();
                let pg_bg = pg.clone();
                tokio::spawn(async move {
                    if crate::fleet::docker::wait_healthy(&pg_bg, &dept, 120_000).await {
                        let slot = crate::fleet::docker::active_slot(&pg_bg, &dept).await;
                        let _ = crate::fleet::docker::prune_bundled_skills(&dept, slot).await;
                    }
                });
            }
            Err(e) => warnings.push(format!("{display_name}: {e}")),
        }
    }
    Ok(ReconcileResult {
        rendered: render.agents.len(),
        started,
        already_running,
        warnings,
    })
}

pub async fn roll_running_agents(
    pg: &PgPool,
    sb: &SecretBox,
) -> Result<(Vec<String>, Vec<String>), String> {
    let render = render_fleet(pg, sb, None).await?;
    let managed: Vec<(String, String)> = sqlx::query_as(
        "select department, display_name from agent_defs where managed and enabled order by slug",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    let running: HashSet<String> = if managed.is_empty() {
        HashSet::new()
    } else {
        crate::fleet::docker::running_departments(
            &managed.iter().map(|m| m.0.clone()).collect::<Vec<_>>(),
        )
        .await?
        .into_iter()
        .collect()
    };
    let mut rolled = Vec::new();
    let mut warnings = render.warnings;
    for (department, display_name) in &managed {
        if !running.contains(department) {
            continue;
        }
        match roll_agent(pg, sb, department).await {
            Ok(None) => rolled.push(display_name.clone()),
            Ok(Some(error)) => warnings.push(error),
            Err(e) => warnings.push(e),
        }
    }
    Ok((rolled, warnings))
}
