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
// is answered by why compose failed:
// - docker's own error (a port already bound, a conflict) with the container
//   in the wrong state: the fixed 500 failure sentence;
// - the CLI killed at its deadline: a 503 that says the outcome is not
//   confirmed. The daemon may still be carrying out what the killed CLI
//   asked — that stop landed about two minutes after the answer, well past
//   the settle window — so "not there yet" is not "failed";
// - docker not answering the re-read at all: the same 503.
//
// EACH COMPOSE VERB RUNS ON A TASK OF ITS OWN ([`own_task`]). A client that
// goes away drops the request's future, and `docker()` kills its CLI with the
// future it belongs to — so a curl that gave up mid-`up` killed compose
// between creating a recreate's container and renaming it, stranding it, and
// left the agent stopped with nothing logged (seen live under heavy IO, where
// the render ahead of it took minutes). Awaiting a spawned task instead, the
// verb finishes and its failure is logged, whoever is still listening.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_defs::agent_def_by_id;
use talaria_api_facades::fleet::create::delete_agent_forever;
use talaria_api_facades::fleet::docker::{
    Observed, fleet_remove, fleet_restart, fleet_stop, fleet_up, is_timeout, observe,
    remove_stopped, settle, up_landed,
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
            if let Err(e) = own_task(&state, &def.department, |pg, d| async move {
                fleet_up(&pg, &d).await
            })
            .await
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
        "stop" => match own_task(&state, &def.department, |pg, d| async move {
            fleet_stop(&pg, &d).await
        })
        .await
        {
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
            match own_task(&state, &def.department, |pg, d| async move {
                fleet_restart(&pg, &d).await
            })
            .await
            {
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
            // Spin down + drop from the fleet. Container removed (a remove
            // that ran out its time after the stop is finished by
            // `settled`); the state volume and the version history stay
            // (re-hire with 'unretire').
            if let Err(e) = sqlx::query(
                "update agent_defs set enabled = false, updated_at = now() where id = $1::uuid",
            )
            .bind(&def.id)
            .execute(&state.pg)
            .await
            {
                return Ok(catch(e.to_string()));
            }
            if let Err(e) = own_task(&state, &def.department, |pg, d| async move {
                fleet_remove(&pg, &d).await
            })
            .await
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
                if let Err(e) = own_task(&state, &def.department, |pg, d| async move {
                    fleet_up(&pg, &d).await
                })
                .await
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

/// Run one compose verb on a task of its own and wait for it (see the
/// header): a client that goes away drops this future, not the verb's.
async fn own_task<F, Fut>(state: &AppState, department: &str, verb: F) -> Result<String, String>
where
    F: FnOnce(sqlx::PgPool, String) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>> + Send + 'static,
{
    tokio::spawn(verb(state.pg.clone(), department.to_string()))
        .await
        .unwrap_or_else(|e| Err(format!("the compose verb's task failed: {e}")))
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

/// Why a failed verb's outcome is not known yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unconfirmed {
    /// Docker would not answer the re-read.
    Silent,
    /// The CLI was killed at its deadline, and the container is not (yet)
    /// what was asked: the daemon may still be carrying the request out.
    StillWorking,
}

/// The answer when a failed verb's outcome is not known yet — the em dash is
/// part of the wire contract, as in [`could_not`].
fn unconfirmed(action: &str, why: Unconfirmed) -> String {
    match why {
        Unconfirmed::Silent => format!(
            "could not confirm the agent's {action} — docker is not answering; the roster shows its state once it does"
        ),
        Unconfirmed::StillWorking => format!(
            "could not confirm the agent's {action} — docker did not finish it in time and may still be working on it; the roster shows its state once it does"
        ),
    }
}

/// How a verb that did not reach what was asked is answered: by why compose
/// failed. `None` is the fixed failure (500) — docker answered, with an
/// error of its own, and the container is in the wrong state. A deadline,
/// or a re-read docker would not answer, leaves the outcome unknown (503).
fn unconfirmed_why(error: &str, last: &Observed) -> Option<Unconfirmed> {
    if matches!(last, Observed::Unreadable) {
        Some(Unconfirmed::Silent)
    } else if is_timeout(error) {
        Some(Unconfirmed::StillWorking)
    } else {
        None
    }
}

/// A compose verb failed and the container did not reach what was asked
/// within the settle window: answered per [`unconfirmed_why`].
fn failed(
    action: &str,
    display_name: &str,
    department: &str,
    error: &str,
    last: &Observed,
) -> Response {
    if let Some(why) = unconfirmed_why(error, last) {
        tracing::warn!(
            "[fleet] {action} of {display_name} ({department}) is unconfirmed: compose failed ({error}) and the container is {}",
            last.describe()
        );
        return house_error(StatusCode::SERVICE_UNAVAILABLE, &unconfirmed(action, why));
    }
    control_failure(action, display_name, department, error)
}

/// Whether a container compose reported failing on is, in fact, what
/// `action` asked for. `before` is the reading taken ahead of a restart.
/// A retire (`rm -sf`) that left a stopped container has still spun the
/// agent down; [`settled`] then removes the leftover.
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
        // A retire's `rm -sf` that ran out its time after the stop leaves
        // the container stopped and not removed, and nothing later sweeps
        // it (each compose call names one service). Finish the remove — an
        // unforced `docker rm`, which refuses a container that runs.
        if action == "retire" && matches!(now, Observed::Stopped { .. }) {
            match remove_stopped(&state.pg, department).await {
                Ok(_) => tracing::warn!(
                    "[fleet] retire of {display_name} ({department}): removed the stopped container compose left"
                ),
                Err(e) => tracing::warn!(
                    "[fleet] retire of {display_name} ({department}) stopped the container but could not remove it ({e}) — deleting the agent removes it, or remove it by hand"
                ),
            }
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
    fn a_verb_that_did_not_land_is_failed_only_on_docker_s_own_error() {
        let timeout = "docker compose …: timed out after 75s";
        let bind = "docker compose failed: failed to bind host port 127.0.0.1:20900/tcp: address already in use";
        // Killed at its deadline, still running when the window closed: the
        // daemon may yet land the stop (it did, two minutes on) — unknown.
        assert_eq!(
            unconfirmed_why(timeout, &running("t1")),
            Some(Unconfirmed::StillWorking)
        );
        assert_eq!(
            unconfirmed_why(
                timeout,
                &Observed::Stopped {
                    id: "c0ffee".into()
                }
            ),
            Some(Unconfirmed::StillWorking)
        );
        // Docker would not describe the container: unknown, whatever failed.
        assert_eq!(
            unconfirmed_why(timeout, &Observed::Unreadable),
            Some(Unconfirmed::Silent)
        );
        assert_eq!(
            unconfirmed_why(bind, &Observed::Unreadable),
            Some(Unconfirmed::Silent)
        );
        // Docker's own error and the container in the wrong state: failed.
        assert_eq!(
            unconfirmed_why(
                bind,
                &Observed::Stopped {
                    id: "c0ffee".into()
                }
            ),
            None
        );
        assert_eq!(unconfirmed_why(bind, &running("t1")), None);
    }

    #[test]
    fn the_wire_sentences_keep_their_shape() {
        assert_eq!(
            could_not("stop"),
            "could not stop the agent — see server logs"
        );
        assert_eq!(
            unconfirmed("stop", Unconfirmed::Silent),
            "could not confirm the agent's stop — docker is not answering; the roster shows its state once it does"
        );
        assert_eq!(
            unconfirmed("restart", Unconfirmed::StillWorking),
            "could not confirm the agent's restart — docker did not finish it in time and may still be working on it; the roster shows its state once it does"
        );
    }
}
