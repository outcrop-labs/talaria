// POST /api/fleet/hires/{id}/retry. Run a FAILED hire again, on the same
// agent — the Retry on the roster's red hire row.
//
// WHY THIS EXISTS. A hire that fails after its create stage (the image would
// not download, compose ran out its time) leaves the agent's def behind: the
// handle and the department are taken, so the create modal refuses a second
// hire of the same agent ("already exists", "already belongs to"), and the
// only way forward was finding Start on a draft card or deleting the draft.
// Measured on rc 65dbae86: both refusals, after a hire failed on a slow
// first pull.
//
// WHO. The same authority Start has over the agent (docs/PERMISSIONS.md,
// "Agent managers"): its managers and admins. A failed hire whose def was
// deleted since has no managers to ask, and its retry CREATES the agent
// again: then the person who ran that hire (the run's owner), if they still
// hold `agents.manage`, or an admin. `agents.manage` alone does not reach an
// agent somebody else manages.
//
// WHAT RUNS. A NEW agent-hire run with the failed run's own input, the
// person who pressed Retry as the actor, and the first run's own `start`.
// Its create stage meets the def the first run wrote and adopts it —
// `create_or_resume`'s already-exists path, which adopts only a def the SAME
// hire wrote (its hirer is among the def's managers, and the input keeps
// that hirer) — then renders, pulls and boots as any hire. A def deleted
// meanwhile is simply created again. The new run's id is derived from the
// failed one's (`md5('agent-hire retry of ' || id)`), so a second Retry of
// the same row — another tab, another admin — is refused at the insert, not
// a second drive racing the first's compose verbs.
//
// REFUSED:
// - 404: no such hire;
// - 403: not this agent's manager (nor, for a deleted def, the hire's owner
//   holding `agents.manage`) and not an admin;
// - 409: a hire that did not fail; one that is not the NEWEST hire of its
//   handle (a later hire, or this row's own retry, answered it — retrying a
//   stale row would rewrite starter skills over an agent that moved on);
//   one already retried; and an agent that has RUN since its hire failed —
//   started by a person, perhaps stopped by one since: that is the person's
//   to start (Start on its card), not the retry's;
// - 503: docker did not say whether the agent has run.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api_facades::fleet::docker::has_run;
use talaria_api_facades::runs::defs::agent_hire::agent_hire_run;
use talaria_api_facades::runs::run::{EnqueueOptions, enqueue};
use talaria_error::{house_error, internal};
use talaria_realtime_watch::RealtimeDeps;
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_work_dispatch::dispatch_deps;

/// The input the retry runs with: the failed hire's own, with the person who
/// pressed Retry as the actor. Everything else — `start`, the hirer who
/// becomes (and already is) the agent's manager, the soul, the skills — is
/// the original's, so the create stage adopts the def that hire wrote.
pub fn retry_input(original: &Value, actor: &str) -> Option<Value> {
    let mut input = original.as_object()?.clone();
    if !input.get("slug").is_some_and(Value::is_string) {
        return None;
    }
    input.insert("actor".into(), Value::String(actor.to_string()));
    Some(Value::Object(input))
}

/// What the route knows of the failed hire and the caller before it asks
/// docker anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryFacts {
    /// The run's state (`error` is the only one a retry runs).
    pub state: String,
    /// No other hire of the same handle is as new as this one.
    pub newest: bool,
    /// The caller manages the agent (admins do), or — its def gone — ran this
    /// hire or is an admin.
    pub may_act: bool,
}

/// Why a retry is refused before docker is asked: the status and sentence.
pub fn refusal(f: &RetryFacts) -> Option<(StatusCode, String)> {
    if !f.may_act {
        return Some((
            StatusCode::FORBIDDEN,
            "only this agent's managers (or an admin) can retry its hire".into(),
        ));
    }
    if f.state != "error" {
        return Some((
            StatusCode::CONFLICT,
            format!(
                "only a failed hire can be retried — this one is {}",
                f.state
            ),
        ));
    }
    if !f.newest {
        return Some((
            StatusCode::CONFLICT,
            "a newer hire of this agent exists — that is the one to look at (or retry)".into(),
        ));
    }
    None
}

/// Why a retry is refused by what docker says of the agent's container:
/// `has_run` is `Some(true)` once it has started since its hire failed.
pub fn refusal_by_container(has_run: Option<bool>, name: &str) -> Option<(StatusCode, String)> {
    match has_run {
        Some(false) => None,
        Some(true) => Some((
            StatusCode::CONFLICT,
            format!(
                "{name} has run since this hire failed, so a retry would start it over its \
                 manager's head — press Start on {name} instead"
            ),
        )),
        None => Some((
            StatusCode::SERVICE_UNAVAILABLE,
            format!("docker did not say whether {name} has run — try again in a moment"),
        )),
    }
}

/// Whether the caller may act on the agent the failed hire was for: an admin;
/// the agent's managers while its def exists (Start's own rule,
/// `manages_agent_slug`); and once the def is gone, the hire's owner — who
/// must still hold `agents.manage`, because a retry then CREATES the agent
/// again, and creating one is that permission's.
pub async fn may_act(
    pg: &PgPool,
    user_id: &str,
    role: &str,
    slug: &str,
    def_exists: bool,
    owner: Option<&str>,
) -> Result<bool, sqlx::Error> {
    if role == "admin" {
        return Ok(true);
    }
    if def_exists {
        return talaria_agent_managers::manages_agent_slug(pg, user_id, role, slug).await;
    }
    if owner != Some(user_id) {
        return Ok(false);
    }
    talaria_users::has_perm(pg, user_id, role, "agents.manage").await
}

/// The failed hire as the retry reads it, in one statement: its input, state
/// and owner; whether it is the NEWEST hire of its handle (no other hire of
/// the slug as new or newer); the retry's derived run id; and whether the
/// agent's def still exists.
#[derive(Debug, Clone, PartialEq)]
pub struct HireRead {
    pub input: Value,
    pub state: String,
    pub owner: Option<String>,
    pub newest: bool,
    pub retry_id: String,
    pub def_exists: bool,
}

pub async fn read_hire(pg: &PgPool, id: &str) -> Result<Option<HireRead>, sqlx::Error> {
    let row: Option<(Value, String, Option<String>, bool, String, bool)> = sqlx::query_as(
        "select r.input, r.state::text, r.owner_user_id::text, \
                not exists (select 1 from runs n where n.kind = 'agent-hire' and n.id <> r.id \
                            and n.input->>'slug' = r.input->>'slug' \
                            and n.created_at >= r.created_at) as newest, \
                md5('agent-hire retry of ' || r.id::text)::uuid::text as retry_id, \
                exists (select 1 from agent_defs d where d.slug = r.input->>'slug') as def_exists \
         from runs r where r.id = $1::uuid and r.kind = 'agent-hire'",
    )
    .bind(id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(
        |(input, state, owner, newest, retry_id, def_exists)| HireRead {
            input,
            state,
            owner,
            newest,
            retry_id,
            def_exists,
        },
    ))
}

/// What the retry will run, once every refusal that needs no docker passed.
#[derive(Debug, Clone, PartialEq)]
pub struct RetryPlan {
    pub input: Value,
    pub retry_id: String,
    pub slug: String,
    pub department: String,
    pub name: String,
    pub def_exists: bool,
    /// Whose hire it was: an agent design spent on it is theirs.
    pub owner: Option<String>,
}

/// The retry up to docker: read the hire, decide who may act, and refuse —
/// or answer the plan. `Err` is the status and sentence the route answers.
pub async fn plan_retry(
    pg: &PgPool,
    user_id: &str,
    role: &str,
    actor: &str,
    id: &str,
) -> Result<RetryPlan, (StatusCode, String)> {
    let store = |what: &str, e: sqlx::Error| {
        tracing::error!("[fleet/hires/retry] {what}: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "the hire could not be read — see server logs".to_string(),
        )
    };
    let hire = read_hire(pg, id)
        .await
        .map_err(|e| store("hire read failed", e))?
        .ok_or((StatusCode::NOT_FOUND, "not found".to_string()))?;
    let input = retry_input(&hire.input, actor).ok_or((
        StatusCode::CONFLICT,
        "this hire's input does not name an agent, so it cannot be run again".to_string(),
    ))?;
    let slug = input["slug"].as_str().unwrap_or_default().to_string();
    let allowed = may_act(
        pg,
        user_id,
        role,
        &slug,
        hire.def_exists,
        hire.owner.as_deref(),
    )
    .await
    .map_err(|e| store("manager read failed", e))?;
    if let Some(refused) = refusal(&RetryFacts {
        state: hire.state,
        newest: hire.newest,
        may_act: allowed,
    }) {
        return Err(refused);
    }
    let department = input["department"].as_str().unwrap_or_default().to_string();
    let name = input["displayName"]
        .as_str()
        .filter(|n| !n.is_empty())
        .unwrap_or(&slug)
        .to_string();
    Ok(RetryPlan {
        input,
        retry_id: hire.retry_id,
        slug,
        department,
        name,
        def_exists: hire.def_exists,
        owner: hire.owner,
    })
}

/// After an enqueue failed: a run with the derived id exists, so this hire
/// was retried already (a second tab, a second admin, a double click past
/// the button's guard) — the 409. Asked of the row rather than read off the
/// error: the run store re-wraps the insert's error, and its
/// unique-violation code does not survive that (measured: two Retries at
/// once got a 200 and a 500).
pub async fn retried_already(pg: &PgPool, retry_id: &str) -> Option<(StatusCode, String)> {
    let taken: bool = sqlx::query_scalar("select exists (select 1 from runs where id = $1::uuid)")
        .bind(retry_id)
        .fetch_one(pg)
        .await
        .unwrap_or(false);
    taken.then(|| {
        (
            StatusCode::CONFLICT,
            "this hire was retried already — its retry is on the roster".to_string(),
        )
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    let actor = talaria_session::actor_of(&user);
    let plan = match plan_retry(&state.pg, &user.id, &user.role, &actor, &id).await {
        Ok(plan) => plan,
        Err((status, why)) => return Ok(house_error(status, &why)),
    };
    // Only a def that exists has a container to have run.
    let ran = if plan.def_exists {
        has_run(&state.pg, &plan.department).await
    } else {
        Some(false)
    };
    if let Some((status, why)) = refusal_by_container(ran, &plan.name) {
        return Ok(house_error(status, &why));
    }

    let Some(redis) = state.redis().await.ok() else {
        return Ok(house_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the hire could not be enqueued: redis is unavailable",
        ));
    };
    let realtime = RealtimeDeps::publish_only(Some(redis.clone()));
    let deps = dispatch_deps(state.pg.clone(), redis, realtime);
    // An agent design spent on the failed hire follows the retry before it
    // is enqueued, so it is consumed when the retry finishes instead of
    // coming back for an agent that by then exists.
    if let Some(owner) = plan.owner.as_deref() {
        talaria_agent_design::follow_retry(&state, owner, &id, &plan.retry_id).await;
    }
    let row = match enqueue(
        agent_hire_run(),
        plan.input,
        EnqueueOptions {
            id: Some(plan.retry_id.clone()),
            owner_user_id: Some(user.id.clone()),
            subject_type: Some("agent-hire".into()),
            subject_id: Some(plan.slug),
            phase: Some("queued".into()),
            start: Some(true),
        },
        &deps,
    )
    .await
    {
        Ok(row) => row,
        Err(e) => {
            if let Some((status, why)) = retried_already(&state.pg, &plan.retry_id).await {
                return Ok(house_error(status, &why));
            }
            return Ok(internal("[fleet/hires/retry] hire enqueue failed", e));
        }
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
    fn a_retry_runs_the_failed_hire_s_own_input_with_the_retrier_as_actor() {
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
        assert_eq!(input["actor"], json!("second@example.com"));
        // The hire's own start — a retry never turns a no-start hire into a boot.
        assert_eq!(input["start"], json!(false));
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
        assert!(!parsed.start);

        assert_eq!(retry_input(&json!({"department": "ops"}), "a"), None);
        assert_eq!(retry_input(&json!("not an object"), "a"), None);
    }

    fn facts(state: &str, newest: bool, may_act: bool) -> RetryFacts {
        RetryFacts {
            state: state.into(),
            newest,
            may_act,
        }
    }

    #[test]
    fn another_member_is_refused_whatever_the_hire_looks_like() {
        for (state, newest) in [("error", true), ("error", false), ("running", true)] {
            let (status, why) = refusal(&facts(state, newest, false)).expect("refused");
            assert_eq!(status, StatusCode::FORBIDDEN, "{state} {newest}");
            assert!(why.contains("managers"), "{why}");
        }
    }

    #[test]
    fn only_a_failed_hire_is_retried() {
        for state in ["queued", "running", "done", "cancelled", "awaiting"] {
            let (status, why) = refusal(&facts(state, true, true)).expect("refused");
            assert_eq!(status, StatusCode::CONFLICT, "{state}");
            assert!(why.contains(state), "{why}");
        }
        assert_eq!(refusal(&facts("error", true, true)), None);
    }

    #[test]
    fn a_stale_row_is_refused_the_newest_hire_answers_for_the_agent() {
        let (status, why) = refusal(&facts("error", false, true)).expect("refused");
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(why.contains("newer hire"), "{why}");
    }

    #[test]
    fn an_agent_that_has_run_since_is_the_person_s_to_start() {
        assert_eq!(refusal_by_container(Some(false), "Sloane"), None);
        let (status, why) = refusal_by_container(Some(true), "Sloane").expect("refused");
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(why.contains("press Start on Sloane"), "{why}");
        let (status, _) = refusal_by_container(None, "Sloane").expect("refused");
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }
}
