// POST /api/fleet/agents/{id}/control. Lifecycle control for one agent —
// this agent's managers, and admins (docs/PERMISSIONS.md, "Agent managers").
//   up | stop | restart   the managed service (renders first on `up`)
//   roll                  recreate the container in place — detached
// `up`/`unretire`/`roll` return IMMEDIATELY; the roster's polled container
// health shows the warm-up ('starting') phase instead of blocking the call.
//
// A FAILED VERB IS ANSWERED BY THE CONTAINER, NOT BY COMPOSE. Under heavy
// IO a `stop` ran out its deadline and answered 500 while the daemon went on
// and stopped the agent all the same. So every compose failure is logged
// with compose's own error (its stderr, or the timeout), the container is
// re-read (talaria-fleet-docker `settle` / `up_landed`), and an agent that
// did reach what was asked answers as the success it is. One that did not
// answers the fixed failure sentence; and when docker itself would not
// answer the re-read (it took minutes to on this host, mid-build), the
// answer says the outcome is unconfirmed rather than that it failed — that
// stop landed three minutes later.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_defs::agent_def_by_id;
use talaria_api_facades::fleet::create::delete_agent_forever;
use talaria_api_facades::fleet::docker::{
    Observed, fleet_remove, fleet_restart, fleet_stop, fleet_up, observe, settle, up_landed,
};
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

    // Docker and pg failure text is operator context, not a user sentence:
    // the answer names the verb, and the log line holds the why — compose's
    // own stderr, which `docker()` carries in the error.
    let catch = |e: String| -> Response {
        control_failure(&action, &def.display_name, &def.department, &e)
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
            if let Err(e) = fleet_up(&state.pg, &def.department).await
                && let Err(last) =
                    up_landed_anyway(&state, &action, &def.display_name, &def.department, &e).await
            {
                return Ok(failed(
                    &action,
                    &def.display_name,
                    &def.department,
                    &e,
                    &last,
                ));
            }
            // Don't block on health — the roster shows the warm-up phase.
            Json(json!({ "ok": true, "warming": true })).into_response()
        }
        "stop" => match fleet_stop(&state.pg, &def.department).await {
            Ok(_) => Json(json!({ "ok": true })).into_response(),
            Err(e) => match settled(
                &state,
                &action,
                &def.display_name,
                &def.department,
                &e,
                None,
            )
            .await
            {
                Ok(_) => Json(json!({ "ok": true })).into_response(),
                Err(last) => failed(&action, &def.display_name, &def.department, &e, &last),
            },
        },
        "restart" => {
            // Quick bounce of the existing container (brief downtime; the
            // same env and image). 'roll' recreates it from the current
            // render instead.
            if !def.managed {
                return Ok(house_error(StatusCode::BAD_REQUEST, "not a managed agent"));
            }
            // The start stamp before, so a restart that did happen can be
            // told from one that did not when compose reports a failure.
            let before = observe(&state.pg, &def.department).await;
            match fleet_restart(&state.pg, &def.department).await {
                Ok(_) => Json(json!({ "ok": true, "warming": true })).into_response(),
                Err(e) => {
                    match settled(
                        &state,
                        &action,
                        &def.display_name,
                        &def.department,
                        &e,
                        Some(&before),
                    )
                    .await
                    {
                        Ok(_) => Json(json!({ "ok": true, "warming": true })).into_response(),
                        Err(last) => failed(&action, &def.display_name, &def.department, &e, &last),
                    }
                }
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
            if let Err(e) = sqlx::query(
                "update agent_defs set enabled = false, updated_at = now() where id = $1::uuid",
            )
            .bind(&def.id)
            .execute(&state.pg)
            .await
            {
                return Ok(catch(e.to_string()));
            }
            if let Err(e) = fleet_remove(&state.pg, &def.department).await
                && let Err(last) = settled(
                    &state,
                    &action,
                    &def.display_name,
                    &def.department,
                    &e,
                    None,
                )
                .await
            {
                return Ok(failed(
                    &action,
                    &def.display_name,
                    &def.department,
                    &e,
                    &last,
                ));
            }
            let sb = match state.secretbox().await {
                Ok(sb) => sb,
                Err(e) => return Ok(catch(e)),
            };
            match render_fleet(&state.pg, &sb).await {
                // the manifest drops it; the persona door re-reads it on each attempt
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
                // flattens to the same operator sentence, and the log keeps it.
                Err(e) => catch(e),
            }
        }
        "unretire" => {
            // Re-hire: re-enable, re-render (manifest + compose pick it back
            // up), and start the managed container from its preserved volume.
            if let Err(e) = sqlx::query(
                "update agent_defs set enabled = true, updated_at = now() where id = $1::uuid",
            )
            .bind(&def.id)
            .execute(&state.pg)
            .await
            {
                return Ok(catch(e.to_string()));
            }
            let sb = match state.secretbox().await {
                Ok(sb) => sb,
                Err(e) => return Ok(catch(e)),
            };
            if let Err(e) = render_fleet(&state.pg, &sb).await {
                return Ok(catch(e));
            }
            if def.managed {
                if let Err(e) = fleet_up(&state.pg, &def.department).await
                    && let Err(last) =
                        up_landed_anyway(&state, &action, &def.display_name, &def.department, &e)
                            .await
                {
                    return Ok(failed(
                        &action,
                        &def.display_name,
                        &def.department,
                        &e,
                        &last,
                    ));
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

/// The catch arm: log what failed and why — the server logs the wire
/// sentence points at — and answer the fixed sentence.
fn control_failure(action: &str, display_name: &str, department: &str, error: &str) -> Response {
    tracing::warn!("[fleet] {action} of {display_name} ({department}) failed: {error}");
    house_error(StatusCode::INTERNAL_SERVER_ERROR, &could_not(action))
}

/// The answer when docker would not say what the container is doing after
/// the verb failed — the em dash is part of the wire contract, as in
/// [`could_not`].
fn unconfirmed(action: &str) -> String {
    format!(
        "could not confirm the agent's {action} — docker is not answering; the roster shows its state once it does"
    )
}

/// A compose verb failed and the container did not reach what was asked
/// within the settle window: the fixed failure sentence when docker said
/// what it is instead (running when a stop was asked, say), the unconfirmed
/// one (503) when docker would not say at all.
fn failed(
    action: &str,
    display_name: &str,
    department: &str,
    error: &str,
    last: &Observed,
) -> Response {
    if matches!(last, Observed::Unreadable) {
        tracing::warn!(
            "[fleet] {action} of {display_name} ({department}) is unconfirmed: compose failed ({error}) and docker did not answer the re-read"
        );
        return house_error(StatusCode::SERVICE_UNAVAILABLE, &unconfirmed(action));
    }
    control_failure(action, display_name, department, error)
}

/// Whether a container compose reported failing on is, in fact, what
/// `action` asked for. `before` is the reading taken ahead of a restart.
/// A retire (`rm -sf`) that left a stopped container has still spun the
/// agent down; the leftover is named in the log.
fn reached(action: &str, before: Option<&Observed>, now: &Observed) -> bool {
    match action {
        "stop" | "retire" => matches!(now, Observed::Stopped { .. } | Observed::Absent),
        "restart" => match (before, now) {
            (
                Some(Observed::Running {
                    started_at: was, ..
                }),
                Observed::Running { started_at, .. },
            ) => started_at != was,
            // It was not running, and now it is: that start is the restart.
            (Some(Observed::Stopped { .. } | Observed::Absent), Observed::Running { .. }) => true,
            _ => false,
        },
        _ => false,
    }
}

/// A stop, restart or remove failed with `error`: re-read the container
/// until it reaches what was asked or the settle window closes. `Ok` with
/// the reading when it did (logged with compose's error, so the log says
/// what compose said and what the container did), `Err` with the last
/// reading when it did not — for [`failed`] to answer by.
async fn settled(
    state: &AppState,
    action: &str,
    display_name: &str,
    department: &str,
    error: &str,
    before: Option<&Observed>,
) -> Result<Observed, Observed> {
    let now = settle(&state.pg, department, error, |o| reached(action, before, o)).await;
    if reached(action, before, &now) {
        tracing::warn!(
            "[fleet] {action} of {display_name} ({department}): compose reported a failure ({error}), but the container is {} — answering by its state",
            now.describe()
        );
        if action == "retire" && matches!(now, Observed::Stopped { .. }) {
            tracing::warn!(
                "[fleet] retire of {display_name} ({department}) stopped the container but did not remove it — remove it by hand"
            );
        }
        return Ok(now);
    }
    tracing::warn!(
        "[fleet] {action} of {display_name} ({department}): after compose's failure the container is {}",
        now.describe()
    );
    Err(now)
}

/// An `up` failed with `error`: whether the agent runs the current render
/// all the same (talaria-fleet-docker `up_landed`), logged either way. `Err`
/// carries a last reading of the container for [`failed`].
async fn up_landed_anyway(
    state: &AppState,
    action: &str,
    display_name: &str,
    department: &str,
    error: &str,
) -> Result<(), Observed> {
    if up_landed(&state.pg, department, error).await {
        tracing::warn!(
            "[fleet] {action} of {display_name} ({department}): compose reported a failure ({error}), but the container runs the current render — answering by its state"
        );
        return Ok(());
    }
    let last = observe(&state.pg, department).await;
    tracing::warn!(
        "[fleet] {action} of {display_name} ({department}): after compose's failure the agent is not running the current render (the container is {})",
        last.describe()
    );
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(at: &str) -> Observed {
        Observed::Running {
            id: "c0ffee".into(),
            started_at: at.into(),
        }
    }

    #[test]
    fn a_failed_verb_is_answered_by_the_state_it_left() {
        let stopped = Observed::Stopped {
            id: "c0ffee".into(),
        };
        // stop: stopped or gone is a stop; still running is not.
        assert!(reached("stop", None, &stopped));
        assert!(reached("stop", None, &Observed::Absent));
        assert!(!reached("stop", None, &running("t1")));
        assert!(!reached("stop", None, &Observed::Unreadable));
        // retire (rm -sf): the same — a stopped leftover no longer serves.
        assert!(reached("retire", None, &Observed::Absent));
        assert!(reached("retire", None, &stopped));
        assert!(!reached("retire", None, &running("t1")));
        // restart: a new start stamp is a restart; the same one is not.
        assert!(reached("restart", Some(&running("t1")), &running("t2")));
        assert!(!reached("restart", Some(&running("t1")), &running("t1")));
        assert!(!reached("restart", Some(&running("t1")), &stopped));
        assert!(reached("restart", Some(&stopped), &running("t2")));
        assert!(reached("restart", Some(&Observed::Absent), &running("t2")));
        // Nothing known before or now is never read as done.
        assert!(!reached(
            "restart",
            Some(&Observed::Unreadable),
            &running("t2")
        ));
        assert!(!reached("restart", None, &running("t2")));
        assert!(!reached(
            "restart",
            Some(&running("t1")),
            &Observed::Unreadable
        ));
        // up and unretire are decided by the render fit, not here.
        assert!(!reached("up", None, &running("t1")));
    }

    #[test]
    fn the_wire_sentences_keep_their_shape() {
        assert_eq!(
            could_not("stop"),
            "could not stop the agent — see server logs"
        );
        assert_eq!(
            unconfirmed("stop"),
            "could not confirm the agent's stop — docker is not answering; the roster shows its state once it does"
        );
    }
}
