// POST /api/fleet/agents/{id}/control — port of
// ui/src/routes/api/fleet.agents.$id.control.ts. Lifecycle control for one
// agent (admin; owners of a personal assistant may up/stop/restart their
// own).
//   up | stop | restart   the managed service (renders first on `up`)
//   roll                  zero-downtime replacement (admin) — detached
// `up`/`unretire`/`roll` return IMMEDIATELY; the roster's polled container
// health shows the warm-up ('starting') phase instead of blocking the call.

use crate::agent_defs::agent_def_by_id;
use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, enum_member, parse};
use crate::error::{house_error, thrown_internal_error};
use crate::fleet_create::delete_agent_forever;
use crate::fleet_docker::{
    active_slot, fleet_remove, fleet_restart, fleet_stop, fleet_up, prune_bundled_skills,
    wait_healthy,
};
use crate::fleet_reconcile::roll_agent;
use crate::fleet_render::render_fleet;
use crate::permissions::has_perm;
use crate::personal_agent::owns_agent;
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn post(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let action = match enum_member(
        obj,
        "action",
        &[
            "up", "stop", "restart", "roll", "retire", "unretire", "delete",
        ],
    ) {
        Ok(a) => a,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let owner_allowed = matches!(action.as_str(), "up" | "stop" | "restart")
        && owns_agent(&state.pg, &user.id, None, Some(&id)).await;
    let perm = has_perm(&state.pg, &user.id, &user.role, "agents.manage")
        .await
        .unwrap_or(false);
    if !perm && !owner_allowed {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let def = match agent_def_by_id(&state.pg, &id).await {
        Ok(Some(d)) => d,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(_) => return thrown_internal_error(),
    };

    // Lifecycle actions are governance-relevant — record them, BEFORE the
    // act (an audit that only logged successes would be a lie).
    if matches!(
        action.as_str(),
        "restart" | "roll" | "retire" | "unretire" | "delete"
    ) {
        let actor = actor_of(&user);
        let audit_action = format!("agent.{action}");
        let target_id = def.id.clone();
        let label = def.display_name.clone();
        let pg = state.pg.clone();
        tokio::spawn(async move {
            log_audit(
                &pg,
                AuditEntry {
                    actor: &actor,
                    action: &audit_action,
                    target_type: "agent",
                    target_id: Some(&target_id),
                    target_label: Some(&label),
                    before: None,
                    after: None,
                },
            )
            .await;
        });
    }

    let catch = |e: String| -> Response {
        // Docker and pg failure text is operator context, not a user
        // sentence — the message names the verb, the logs hold the why.
        let _ = e;
        house_error(StatusCode::INTERNAL_SERVER_ERROR, &could_not(&action))
    };

    match action.as_str() {
        "up" => {
            if !def.managed {
                return house_error(StatusCode::BAD_REQUEST, "not a managed agent");
            }
            let sb = match state.secretbox().await {
                Ok(sb) => sb,
                Err(e) => return catch(e),
            };
            if let Err(e) = render_fleet(&state.pg, &sb, None).await {
                return catch(e);
            }
            if let Err(e) = fleet_up(&state.pg, &def.department).await {
                return catch(e);
            }
            // Don't block on health — the roster shows the warm-up phase.
            spawn_health_prune(&state.pg, &def.department);
            Json(json!({ "ok": true, "warming": true })).into_response()
        }
        "stop" => match fleet_stop(&state.pg, &def.department).await {
            Ok(_) => Json(json!({ "ok": true })).into_response(),
            Err(e) => catch(e),
        },
        "restart" => {
            // Quick bounce (brief downtime; in-flight replies drop). For a
            // no-downtime reboot use 'roll'.
            if !def.managed {
                return house_error(StatusCode::BAD_REQUEST, "not a managed agent");
            }
            match fleet_restart(&state.pg, &def.department).await {
                Ok(_) => Json(json!({ "ok": true, "warming": true })).into_response(),
                Err(e) => catch(e),
            }
        }
        "roll" => {
            // Zero-downtime replacement — fresh container, old one drains.
            // Long (health wait + drain), so it runs detached; the roster's
            // health polling tells the story.
            if !perm {
                return house_error(StatusCode::FORBIDDEN, "forbidden");
            }
            if !def.managed {
                return house_error(StatusCode::BAD_REQUEST, "not a managed agent");
            }
            if let Ok(sb) = state.secretbox().await {
                let pg = state.pg.clone();
                let dept = def.department.clone();
                tokio::spawn(async move {
                    let _ = roll_agent(&pg, &sb, &dept).await;
                });
            }
            Json(json!({ "ok": true, "rolling": true })).into_response()
        }
        "retire" => {
            // Spin down + drop from the fleet. Container removed; the state
            // volume and the version history stay (re-hire with 'unretire').
            if sqlx::query(
                "update agent_defs set enabled = false, updated_at = now() where id = $1",
            )
            .bind(&def.id)
            .execute(&state.pg)
            .await
            .is_err()
            {
                return catch(String::new());
            }
            if let Err(e) = fleet_remove(&state.pg, &def.department).await {
                return catch(e);
            }
            let sb = match state.secretbox().await {
                Ok(sb) => sb,
                Err(e) => return catch(e),
            };
            match render_fleet(&state.pg, &sb, None).await {
                // manifest drops it; bridge hot-reloads
                Ok(render) => Json(json!({ "ok": true, "render": render.agents })).into_response(),
                Err(e) => catch(e),
            }
        }
        "delete" => {
            // Permanent: def + versions + secrets + rendered files + (for
            // created agents) the state volume. Admin only, retired only.
            if !perm {
                return house_error(StatusCode::FORBIDDEN, "forbidden");
            }
            let sb = match state.secretbox().await {
                Ok(sb) => sb,
                Err(e) => return catch(e),
            };
            match delete_agent_forever(&state.pg, &sb, &def.id).await {
                Ok(removed_volume) => {
                    Json(json!({ "ok": true, "removedVolume": removed_volume })).into_response()
                }
                // Includes deleteAgentForever's own refusals — the TS catch
                // flattens every throw to the same operator sentence.
                Err(_) => catch(String::new()),
            }
        }
        "unretire" => {
            // Re-hire: re-enable, re-render (manifest + compose pick it back
            // up), and start the managed container from its preserved volume.
            if sqlx::query("update agent_defs set enabled = true, updated_at = now() where id = $1")
                .bind(&def.id)
                .execute(&state.pg)
                .await
                .is_err()
            {
                return catch(String::new());
            }
            let sb = match state.secretbox().await {
                Ok(sb) => sb,
                Err(e) => return catch(e),
            };
            if let Err(e) = render_fleet(&state.pg, &sb, None).await {
                return catch(e);
            }
            if def.managed {
                if let Err(e) = fleet_up(&state.pg, &def.department).await {
                    return catch(e);
                }
                spawn_health_prune(&state.pg, &def.department);
                return Json(json!({ "ok": true, "warming": true })).into_response();
            }
            Json(json!({ "ok": true })).into_response()
        }
        _ => thrown_internal_error(),
    }
}

/// waitHealthy(department).then(ok => ok && pruneBundledSkills(department))
/// with the TS defaults — 120s health window, slot resolved per call.
fn spawn_health_prune(pg: &sqlx::PgPool, department: &str) {
    let pg = pg.clone();
    let dept = department.to_string();
    tokio::spawn(async move {
        if wait_healthy(&pg, &dept, 120_000).await {
            let slot = active_slot(&pg, &dept).await;
            prune_bundled_skills(&dept, slot).await;
        }
    });
}

/// The catch arm's sentence — an em dash, verbatim from the TS template
/// literal.
fn could_not(action: &str) -> String {
    format!("could not {action} the agent — see server logs")
}
