// GET /api/fleet/hires. What the roster shows while an agent-hire run
// works: every live hire, plus the recently-finished ones long enough for
// the surface to see the transition (and a failure's sentence) before the
// row goes away.
//
// An UNANSWERED failure stays, however old: the newest hire of its handle,
// failed, with the agent's def still there — a draft its Retry can still
// finish. Hires that failed before an upgrade (on main, a cold host's first
// hire always did) are exactly those, and their people had no row to retry
// from once ten minutes passed. The roster drops such a row once the agent
// has run (`hiresToShow`), and the retry route refuses one that has.
//
// The getter call is for REGISTRATION only: a process that lists hires can
// also be the process a reclaim sweep asks to resume one, and a kind this
// route never registered would be a run nothing can drive.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::Value;
use talaria_agent_auth::epoch_ms_to_iso;
use talaria_api_facades::runs::defs::agent_hire::agent_hire_run;
use talaria_error::internal;
use talaria_session::require_perm;
use talaria_state::AppState;

/// The hire as the roster reads it — fields in this order, camelCase on
/// the wire.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentHireView {
    id: String,
    name: String,
    slug: String,
    department: String,
    start: bool,
    state: String,
    phase: String,
    error: Option<String>,
    created_at: String,
}

#[derive(Serialize)]
struct HiresBody {
    hires: Vec<AgentHireView>,
}

/// Which `runs` rows the roster lists, as a filter on `runs` (a live-DB test
/// reads it on its own, without the list's ordering and limit): every live
/// hire, any hire that ended in the last ten minutes, and an UNANSWERED
/// failure however old — failed, the newest hire of its handle, its def still
/// there.
macro_rules! listed {
    () => {
        "kind = 'agent-hire' \
         and (state in ('queued', 'running') \
              or (state in ('done', 'error') and updated_at > now() - interval '10 minutes') \
              or (state = 'error' \
                  and exists (select 1 from agent_defs d where d.slug = runs.input->>'slug') \
                  and not exists (select 1 from runs n where n.kind = 'agent-hire' \
                                  and n.id <> runs.id \
                                  and n.input->>'slug' = runs.input->>'slug' \
                                  and n.created_at >= runs.created_at)))"
    };
}

/// [`listed!`] as a constant, for the live-DB test.
pub const HIRES_LISTED: &str = listed!();

const HIRES_SQL: &str = concat!(
    "select id::text, input, state::text, phase, error, \
            (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms \
     from runs where ",
    listed!(),
    " order by created_at desc limit 12"
);

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_perm(&state, &headers, "agents.manage").await?;

    // Registration touch (see the module header) — before the read, so this
    // process answers a reclaim sweep the same boot it answered the roster.
    let _ = agent_hire_run();

    type HireRow = (String, Value, String, Option<String>, Option<String>, i64);
    let rows: Result<Vec<HireRow>, sqlx::Error> =
        sqlx::query_as(HIRES_SQL).fetch_all(&state.pg).await;
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return Ok(internal("[fleet] hires query failed", e)),
    };

    let hires = rows
        .into_iter()
        .map(|(id, input, state, phase, error, created_ms)| {
            hire_view(id, &input, state, phase, error, created_ms)
        })
        .collect();
    Ok(Json(HiresBody { hires }).into_response())
}

/// Each input field is `??`'d to its default — a missing key reads as the
/// default and never fails the list. A null phase reads as the empty
/// string (plan-drafts' precedent).
fn hire_view(
    id: String,
    input: &Value,
    state: String,
    phase: Option<String>,
    error: Option<String>,
    created_ms: i64,
) -> AgentHireView {
    let member = |key: &str| input.get(key).and_then(Value::as_str);
    AgentHireView {
        id,
        name: member("displayName")
            .or_else(|| member("slug"))
            .unwrap_or("the new agent")
            .to_string(),
        slug: member("slug").unwrap_or_default().to_string(),
        department: member("department").unwrap_or_default().to_string(),
        start: input.get("start").and_then(Value::as_bool).unwrap_or(true),
        state,
        phase: phase.unwrap_or_default(),
        error,
        created_at: epoch_ms_to_iso(created_ms),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The projection is pure over a row's parts — the `??` chain, pinned the
    // way the roster reads it (a displayName wins over the slug, a slug-less
    // input is still "the new agent", start defaults true).
    #[test]
    fn the_projection_defaults_every_input_field_like_the_ts_map() {
        let input: Value = serde_json::json!({
            "displayName": "Scout",
            "slug": "scout",
            "department": "research"
        });
        let v = hire_view("s".into(), &input, "s".into(), None, None, 0);
        assert_eq!(
            serde_json::to_value(&v).unwrap(),
            serde_json::json!({
                "id": "s",
                "name": "Scout",
                "slug": "scout",
                "department": "research",
                "start": true,
                "state": "s",
                "phase": "",
                "error": null,
                "createdAt": "1970-01-01T00:00:00.000Z"
            })
        );

        // No displayName → the slug; no slug at all → the fixed phrase; a
        // carried phase and error survive; start false stays false.
        let bare: Value = serde_json::json!({ "department": "ops", "start": false });
        let v = hire_view(
            "s".into(),
            &bare,
            "s".into(),
            Some("boot".into()),
            Some("timed out".into()),
            0,
        );
        let back: serde_json::Value = serde_json::to_value(&v).unwrap();
        assert_eq!(back["name"], "the new agent");
        assert_eq!(back["slug"], "");
        assert_eq!(back["department"], "ops");
        assert_eq!(back["start"], false);
        assert_eq!(back["phase"], "boot");
        assert_eq!(back["error"], "timed out");

        // The empty string is NOT nullish: a blank displayName is the name.
        let blank: Value = serde_json::json!({ "displayName": "", "slug": "relay" });
        let v = hire_view("s".into(), &blank, "s".into(), None, None, 0);
        let back: serde_json::Value = serde_json::to_value(&v).unwrap();
        assert_eq!(back["name"], "");
    }
}
