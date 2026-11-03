// POST /api/fleet/hires/{id}/retry. Run a FAILED hire again, on the same
// agent — the Retry on the roster's red hire row. Admin-level like the hire
// itself (`agents.manage`).
//
// WHY THIS EXISTS. A hire that fails after its create stage (the image would
// not download, compose ran out its time) leaves the agent's def behind: the
// handle and the department are taken, so the create modal refuses a second
// hire of the same agent ("already exists", "already belongs to"), and the
// only way forward was finding Start on a draft card or deleting the draft.
// Measured on rc 65dbae86: both refusals, after a hire failed on a slow
// first pull.
//
// WHAT RUNS. A NEW agent-hire run with the failed run's own input, with
// `start` on (a retry means booting it) and the person who pressed Retry as
// the actor. Its create stage meets the def the first run wrote and adopts
// it — `create_or_resume`'s already-exists path, which adopts only a def the
// SAME hire wrote (its hirer is among the def's managers, and the input
// keeps that hirer) — then renders, pulls and boots as any hire. A def that
// was deleted meanwhile is simply created again. The failed run's row stays
// as it was: the roster hides it once a newer hire of the same agent exists.
//
// REFUSED: a run that is not a failed hire (404 for none, 409 for one still
// going or one that finished), and a retry while another hire of the same
// handle is queued or running — two drives of one agent's boot would race
// each other's compose verbs.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_api_facades::runs::defs::agent_hire::agent_hire_run;
use talaria_api_facades::runs::run::{EnqueueOptions, enqueue};
use talaria_error::{house_error, internal};
use talaria_realtime_watch::RealtimeDeps;
use talaria_session::require_perm;
use talaria_state::AppState;
use talaria_work_dispatch::dispatch_deps;

/// The input the retry runs with: the failed hire's own, booting, with the
/// person who pressed Retry as the actor. Everything else — the hirer who
/// becomes (and already is) the agent's manager, the soul, the skills — is
/// the original's, so the create stage adopts the def that hire wrote.
pub fn retry_input(original: &Value, actor: &str) -> Option<Value> {
    let mut input = original.as_object()?.clone();
    if !input.get("slug").is_some_and(Value::is_string) {
        return None;
    }
    input.insert("start".into(), Value::Bool(true));
    input.insert("actor".into(), Value::String(actor.to_string()));
    Some(Value::Object(input))
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_perm(&state, &headers, "agents.manage").await?;
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    let row: Option<(Value, String)> = match sqlx::query_as(
        "select input, state::text from runs where id = $1::uuid and kind = 'agent-hire'",
    )
    .bind(&id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => return Ok(internal("[fleet/hires/retry] hire read failed", e)),
    };
    let Some((original, run_state)) = row else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    if run_state != "error" {
        return Ok(house_error(
            StatusCode::CONFLICT,
            &format!("only a failed hire can be retried — this one is {run_state}"),
        ));
    }
    let actor = talaria_session::actor_of(&user);
    let Some(input) = retry_input(&original, &actor) else {
        return Ok(house_error(
            StatusCode::CONFLICT,
            "this hire's input does not name an agent, so it cannot be run again",
        ));
    };
    let slug = input["slug"].as_str().unwrap_or_default().to_string();
    let live: Option<(i32,)> = match sqlx::query_as(
        "select 1 from runs where kind = 'agent-hire' and state in ('queued', 'running') \
         and input->>'slug' = $1 limit 1",
    )
    .bind(&slug)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => return Ok(internal("[fleet/hires/retry] live-hire check failed", e)),
    };
    if live.is_some() {
        return Ok(house_error(
            StatusCode::CONFLICT,
            &format!(
                "\"{slug}\" is being hired already — its row on the roster shows how far it got"
            ),
        ));
    }

    let Some(redis) = state.redis().await.ok() else {
        return Ok(house_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the hire could not be enqueued: redis is unavailable",
        ));
    };
    let realtime = RealtimeDeps::publish_only(Some(redis.clone()));
    let deps = dispatch_deps(state.pg.clone(), redis, realtime);
    let row = match enqueue(
        agent_hire_run(),
        input,
        EnqueueOptions {
            id: Some(uuid::Uuid::new_v4().to_string()),
            owner_user_id: Some(user.id.clone()),
            subject_type: Some("agent-hire".into()),
            subject_id: Some(slug),
            phase: Some("queued".into()),
            start: Some(true),
        },
        &deps,
    )
    .await
    {
        Ok(row) => row,
        Err(e) => return Ok(internal("[fleet/hires/retry] hire enqueue failed", e)),
    };
    Ok(Json(json!({
        "ok": true,
        "hire": { "id": row.id, "state": row.state, "phase": row.phase }
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_retry_runs_the_failed_hire_s_own_input_booting_with_the_retrier_as_actor() {
        let original = json!({
            "slug": "sloane",
            "department": "research",
            "displayName": "Sloane",
            "soul": "# Sloane",
            "skills": [{"name": "weekly", "content": "x"}],
            "start": false,
            "actor": "first@example.com",
            "managerUserId": "00000000-0000-0000-0000-00000000beef"
        });
        let input = retry_input(&original, "second@example.com").unwrap();
        assert_eq!(input["start"], json!(true));
        assert_eq!(input["actor"], json!("second@example.com"));
        // The hirer stays: it is what lets the create stage adopt the def
        // the first run wrote.
        assert_eq!(
            input["managerUserId"],
            json!("00000000-0000-0000-0000-00000000beef")
        );
        for k in ["slug", "department", "displayName", "soul", "skills"] {
            assert_eq!(input[k], original[k], "{k}");
        }
        // And it still parses as the run's input.
        let parsed: talaria_api_facades::runs::defs::agent_hire::AgentHireInput =
            serde_json::from_value(input).expect("the retry input parses as a hire's");
        assert!(parsed.start);

        assert_eq!(retry_input(&json!({"department": "ops"}), "a"), None);
        assert_eq!(retry_input(&json!("not an object"), "a"), None);
    }
}
