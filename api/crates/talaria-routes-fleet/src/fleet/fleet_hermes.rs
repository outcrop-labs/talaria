// /api/fleet/hermes — the agent image's Hermes stable channel. Admin.
//   GET   where the channel stands: following stable, pinned to a version,
//         or off because the operator chose the image (HERMES_IMAGE, or a
//         chassis image of their own) and why; the Hermes version each agent
//         runs; the last check of stable and when the next is due; the
//         maintenance window in the instance's time zone; the rollout in
//         flight or the last one (its phase, the agents moved and passed
//         over); every image this instance has seen, with its status.
//   POST  {action}: `check` checks stable now; `roll` runs the rollout now,
//         outside the maintenance window and before its minimum time on
//         stable; `rollback` moves every agent back to the previous version
//         (a version that came from stable is marked bad, a pinned one is
//         not); `pin` ({digest} or {version}: vX.Y.Z) holds every agent on a
//         version not marked bad, canarying one that never ran here first;
//         `unpin` follows stable again; `allow` ({digest}) lifts a version's
//         ban; `window` ({start, end}, HH:MM) sets the maintenance window.
//         Answers `{ok, message, status}`, the status as GET answers it; a
//         rollout runs on after the answer. A state row that cannot be read
//         is a 409 that says so, and nothing is changed.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{optional_enum_member, optional_string_member, parse};
use talaria_error::{house_error, object_or_400};
use talaria_hermes_channel::ChannelDeps;
use talaria_session::{actor_of, require_admin, secretbox_or_500};
use talaria_state::AppState;

const ACTIONS: &[&str] = &[
    "check", "roll", "rollback", "pin", "unpin", "allow", "window",
];

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    match talaria_hermes_channel::status(&state.pg).await {
        Ok(status) => Ok(Json(json!({ "ok": true, "status": status })).into_response()),
        // An unreadable state row (or a store that will not answer) says
        // why, so the panel can: nothing is read as a default.
        Err(e) => {
            tracing::warn!("[fleet] hermes channel status: {e}");
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
                "action is required (check, roll, rollback, pin, unpin, allow, window)",
            ));
        }
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let actor = actor_of(&user);
    let sb = secretbox_or_500(&state, "[fleet] secretbox failed").await?;
    let deps = ChannelDeps {
        pg: state.pg.clone(),
        sb,
        notify: talaria_notify::NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok()),
    };
    let mut target: Option<String> = None;
    let done: Result<String, String> = match action.as_str() {
        "check" => {
            talaria_hermes_channel::check_now(&deps)
                .await
                .map(|c| match (c.error, c.digest) {
                    (Some(e), _) => format!("the check failed: {e}"),
                    (None, Some(d)) => format!(
                        "stable is {}",
                        talaria_hermes_images::label(c.version.as_deref(), &d)
                    ),
                    (None, None) => "the check found nothing".into(),
                })
        }
        "roll" => talaria_hermes_channel::roll_now(&deps).await,
        "rollback" => talaria_hermes_channel::roll_back(&deps, &actor).await,
        "pin" => match (
            optional_string_member(obj, "digest", 80),
            optional_string_member(obj, "version", 16),
        ) {
            (Ok(d), Ok(v)) if d.is_some() || v.is_some() => {
                target = d.clone().or(v.clone());
                talaria_hermes_channel::pin(&deps, d.as_deref(), v.as_deref(), &actor).await
            }
            (Err(msg), _) | (_, Err(msg)) => Err(msg),
            _ => Err("digest or version is required to pin".into()),
        },
        "allow" => match optional_string_member(obj, "digest", 80) {
            Ok(Some(d)) => {
                target = Some(d.clone());
                talaria_hermes_channel::allow(&deps, &d, &actor).await
            }
            Ok(None) => Err("digest is required to allow a version again".into()),
            Err(msg) => Err(msg),
        },
        "unpin" => talaria_hermes_channel::pin(&deps, None, None, &actor).await,
        "window" => match (
            optional_string_member(obj, "start", 5),
            optional_string_member(obj, "end", 5),
        ) {
            (Ok(Some(start)), Ok(Some(end))) => {
                talaria_hermes_channel::set_window(&deps, &start, &end)
                    .await
                    .map(|w| format!("the maintenance window is {}–{}", w.start, w.end))
            }
            (Err(msg), _) | (_, Err(msg)) => Err(msg),
            _ => Err("start and end are required (HH:MM)".into()),
        },
        _ => unreachable!("the enum member already gated the action"),
    };
    let message = match done {
        Ok(m) => m,
        Err(e) => return Ok(house_error(StatusCode::CONFLICT, &e)),
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor,
            action: &format!("fleet.hermes_{action}"),
            target_type: "hermes_image",
            target_id: target.as_deref(),
            target_label: None,
            before: None,
            after: Some(json!({ "message": message })),
        },
    )
    .await;
    let status = match talaria_hermes_channel::status(&state.pg).await {
        Ok(s) => s,
        Err(_) => Value::Null,
    };
    Ok(Json(json!({ "ok": true, "message": message, "status": status })).into_response())
}
