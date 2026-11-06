// /api/workbench/harnesses — the workbench harness channel: Oh My Pi and the
// harness's own bun, kept current from upstream (talaria-workbench-updates;
// docs/UPDATES.md, "The workbench harnesses"). Admin.
//   GET   per harness: the version every new job runs (verified here, or the
//         compiled bootstrap), the previous one a volume falls back to, the
//         pin, the newest candidate and when its soak ends, what is held and
//         why, the last check and the line saying why the newest release is
//         not what jobs run yet; each workbench volume's verified pair; the
//         next check; the history.
//   POST  {action}: `check` reads every harness's upstream now; `update`
//         ({tool}) adopts the newest release seen without its soak; `pin`
//         ({tool, version}: x.y.z) holds every new job on that release, up or
//         down, smoke-testing it first; `unpin` ({tool}) follows upstream
//         again; `allow` ({tool, version}) lifts a hold; `reset` sets an
//         unreadable state aside, kept as it was. `tool` is `omp` or `bun`.
//         Answers `{ok, message, status}`; the channel's next step, within a
//         minute, smoke-tests and adopts. A state row that cannot be read is
//         a 409 that says so (only `reset` acts on it), and so is an action
//         that cannot be done (a version upstream does not have, a release
//         that is not held).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{optional_enum_member, optional_string_member, parse};
use talaria_error::{house_error, object_or_400};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;
use talaria_workbench_updates as updates;

const ACTIONS: &[&str] = &["check", "update", "pin", "unpin", "allow", "reset"];
const TOOLS: &[&str] = &["omp", "bun"];

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    match updates::status(&state.pg).await {
        Ok(status) => Ok(Json(json!({ "ok": true, "status": status })).into_response()),
        Err(e) => {
            tracing::warn!("[workbench] harness channel status: {e}");
            Ok(house_error(StatusCode::CONFLICT, &e))
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let action = match optional_enum_member(obj, "action", ACTIONS) {
        Ok(Some(a)) => a,
        Ok(None) => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "action is required (check, update, pin, unpin, allow, reset)",
            ));
        }
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let tool = match optional_enum_member(obj, "tool", TOOLS) {
        Ok(t) => t,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let version = match optional_string_member(obj, "version", 16) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let needs_tool = matches!(action.as_str(), "update" | "pin" | "unpin" | "allow");
    if needs_tool && tool.is_none() {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "tool is required (omp or bun)",
        ));
    }
    let needs_version = matches!(action.as_str(), "pin" | "allow");
    if needs_version && version.is_none() {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "version is required (x.y.z)",
        ));
    }
    let actor = actor_of(&user);
    let ops = updates::Live {
        pg: state.pg.clone(),
        notify: talaria_notify::NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok()),
    };
    let t = tool.clone().unwrap_or_default();
    let v = version.clone().unwrap_or_default();
    let done: Result<String, String> = match action.as_str() {
        "check" => updates::check_now(&ops).await,
        "update" => updates::update_now(&ops, &t, &actor).await,
        "pin" => updates::pin(&ops, &t, &v, &actor).await,
        "unpin" => updates::unpin(&ops, &t, &actor).await,
        "allow" => updates::allow(&ops, &t, &v, &actor).await,
        "reset" => updates::reset(&state.pg, &actor).await,
        _ => unreachable!("the enum member already gated the action"),
    };
    let message = match done {
        Ok(m) => m,
        Err(e) => return Ok(house_error(StatusCode::CONFLICT, &e)),
    };
    let target = match (&tool, &version) {
        (Some(t), Some(v)) => Some(format!("{t}@{v}")),
        (Some(t), None) => Some(t.clone()),
        _ => None,
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor,
            action: &format!("workbench.harness_{action}"),
            target_type: "workbench_harness",
            target_id: target.as_deref(),
            target_label: None,
            before: None,
            after: Some(json!({ "message": message })),
        },
    )
    .await;
    let status = updates::status(&state.pg).await.unwrap_or(Value::Null);
    Ok(Json(json!({ "ok": true, "message": message, "status": status })).into_response())
}
