// POST /api/fleet/agents/{id}/control. Lifecycle control for one agent —
// this agent's managers, and admins (docs/PERMISSIONS.md, "Agent managers").
//   up | stop | restart   the managed service (renders first on `up`)
//   roll                  recreate the container in place — detached
// `up`/`unretire`/`roll` return IMMEDIATELY; the roster's polled container
// health shows the warm-up ('starting') phase instead of blocking the call.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_defs::agent_def_by_id;
use talaria_api_facades::fleet::create::delete_agent_forever;
use talaria_api_facades::fleet::docker::{fleet_remove, fleet_restart, fleet_stop, fleet_up};
use talaria_api_facades::fleet::reconcile::roll_agent;
use talaria_api_facades::fleet::render::render_fleet;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{enum_member, parse};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_agent_manager};
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    // The gate runs before the body parses: lifecycle is this agent's
    // managers' to drive, whatever they asked for.
    let user = require_agent_manager(&state, &headers, &id).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let action = match enum_member(
        obj,
        "action",
        &[
            "up", "stop", "restart", "roll", "retire", "unretire", "delete",
        ],
    ) {
        Ok(a) => a,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let def = match agent_def_by_id(&state.pg, &id).await {
        Ok(Some(d)) => d,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[fleet] agent_def_by_id failed", e)),
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
    Ok(match action.as_str() {
        "up" => {
            if !def.managed {
                return Ok(house_error(StatusCode::BAD_REQUEST, "not a managed agent"));
            }
            let sb = match state.secretbox().await {
                Ok(sb) => sb,
                Err(e) => return Ok(catch(e)),
            };
            if let Err(e) = render_fleet(&state.pg, &sb).await {
                return Ok(catch(e));
            }
            if let Err(e) = fleet_up(&state.pg, &def.department).await {
                return Ok(catch(e));
            }
            // Don't block on health — the roster shows the warm-up phase.
            Json(json!({ "ok": true, "warming": true })).into_response()
        }
        "stop" => match fleet_stop(&state.pg, &def.department).await {
            Ok(_) => Json(json!({ "ok": true })).into_response(),
            Err(e) => catch(e),
        },
        "restart" => {
            // Quick bounce of the existing container (brief downtime; the
            // same env and image). 'roll' recreates it from the current
            // render instead.
            if !def.managed {
                return Ok(house_error(StatusCode::BAD_REQUEST, "not a managed agent"));
            }
            match fleet_restart(&state.pg, &def.department).await {
                Ok(_) => Json(json!({ "ok": true, "warming": true })).into_response(),
                Err(e) => catch(e),
            }
        }
        "roll" => {
            // Recreate in place — the container stops gracefully and a fresh
            // one starts from the current render. Long (pull, stop, health
            // wait), so it runs detached; the roster's health polling tells
            // the story.
            if !def.managed {
                return Ok(house_error(StatusCode::BAD_REQUEST, "not a managed agent"));
            }
            // One roll per department at a time: `roll_agent` holds the
            // lock itself (in-process set + Postgres advisory lock), so a
            // second click lands as its soft refusal in the audit trail —
            // but a caller who can be told now is told now.
            if talaria_api_facades::fleet::reconcile::is_rolling(&def.department) {
                return Ok(house_error(
                    StatusCode::CONFLICT,
                    "this agent is already rolling — wait for it to finish",
                ));
            }
            // The roll records itself in the fleet-wide registry (start and
            // verdict), which is what /api/fleet/roll shows; the Redis
            // mirror behind it is wired here as well as there, so a
            // manager's roll on an instance nobody pressed "roll all" on
            // still reaches every other instance's picture.
            super::fleet_roll::wire_roll_sink(&state);
            if let Ok(sb) = state.secretbox().await {
                let pg = state.pg.clone();
                let dept = def.department.clone();
                let target_id = def.id.clone();
                let label = def.display_name.clone();
                let actor = actor_of(&user);
                tokio::spawn(async move {
                    // THE VERDICT HAS NOWHERE ELSE TO GO. A roll is detached
                    // and the caller already got `{"rolling": true}`, so a
                    // step that throws and a soft refusal both used to
                    // vanish: the operator saw a roll that never changed
                    // anything and no reason for it. The audit entry is the
                    // same shape mcp-apply writes when its own roll fails.
                    let error = match roll_agent(&pg, &sb, &dept).await {
                        Ok(None) => return,
                        Ok(Some(warning)) => warning,
                        Err(thrown) => thrown,
                    };
                    tracing::warn!("[fleet] roll of {label} ({dept}) failed: {error}");
                    log_audit(
                        &pg,
                        AuditEntry {
                            actor: &actor,
                            action: "agent.roll_failed",
                            target_type: "agent",
                            target_id: Some(&target_id),
                            target_label: Some(&label),
                            before: None,
                            after: Some(json!({ "error": error })),
                        },
                    )
                    .await;
                });
            }
            Json(json!({ "ok": true, "rolling": true })).into_response()
        }
        "retire" => {
            // Spin down + drop from the fleet. Container removed; the state
            // volume and the version history stay (re-hire with 'unretire').
            if sqlx::query(
                "update agent_defs set enabled = false, updated_at = now() where id = $1::uuid",
            )
            .bind(&def.id)
            .execute(&state.pg)
            .await
            .is_err()
            {
                return Ok(catch(String::new()));
            }
            if let Err(e) = fleet_remove(&state.pg, &def.department).await {
                return Ok(catch(e));
            }
            let sb = match state.secretbox().await {
                Ok(sb) => sb,
                Err(e) => return Ok(catch(e)),
            };
            match render_fleet(&state.pg, &sb).await {
                // manifest drops it; bridge hot-reloads
                Ok(render) => Json(json!({ "ok": true, "render": render.agents })).into_response(),
                Err(e) => catch(e),
            }
        }
        "delete" => {
            // Permanent: def + versions + secrets + rendered files + (for
            // created agents) the state volume. Retired agents only — the
            // agent's managers, like every other act on this route.
            let sb = match state.secretbox().await {
                Ok(sb) => sb,
                Err(e) => return Ok(catch(e)),
            };
            match delete_agent_forever(&state.pg, &sb, &def.id).await {
                Ok(removed_volume) => {
                    Json(json!({ "ok": true, "removedVolume": removed_volume })).into_response()
                }
                // Includes delete_agent_forever's own refusals — every throw
                // flattens to the same operator sentence.
                Err(_) => catch(String::new()),
            }
        }
        "unretire" => {
            // Re-hire: re-enable, re-render (manifest + compose pick it back
            // up), and start the managed container from its preserved volume.
            if sqlx::query(
                "update agent_defs set enabled = true, updated_at = now() where id = $1::uuid",
            )
            .bind(&def.id)
            .execute(&state.pg)
            .await
            .is_err()
            {
                return Ok(catch(String::new()));
            }
            let sb = match state.secretbox().await {
                Ok(sb) => sb,
                Err(e) => return Ok(catch(e)),
            };
            if let Err(e) = render_fleet(&state.pg, &sb).await {
                return Ok(catch(e));
            }
            if def.managed {
                if let Err(e) = fleet_up(&state.pg, &def.department).await {
                    return Ok(catch(e));
                }
                return Ok(Json(json!({ "ok": true, "warming": true })).into_response());
            }
            Json(json!({ "ok": true })).into_response()
        }
        _ => internal("[fleet] control: unknown action", &action),
    })
}

/// The catch arm's sentence — the em dash is part of the wire contract.
fn could_not(action: &str) -> String {
    format!("could not {action} the agent — see server logs")
}
