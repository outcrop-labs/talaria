// /api/dms. Find-or-create a direct message (rides the channel machinery:
// same messages, SSE feed, and composer as everything else).
//
//   POST { userId }                       → the DM with that person.
//   POST { userIds, agents?, name? }      → the "New message" pane: a DM with
//     any mix of people and agents, deduped on the whole set — picking the
//     same participants again reopens the same conversation. `name` is
//     optional and labels it (otherwise the UI shows the members' names).
//     One person and no agents is the ordinary DM; one agent alone is that
//     agent's own conversation, which this route refuses.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use talaria_api_facades::fleet::usable_agent_gate;
use talaria_body::{optional_max_string_member, optional_string_array_member, uuid_member};
use talaria_channels::{ensure_dm, ensure_group_dm};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

/// Most participants a DM takes besides the creator — past that it's a
/// channel, and a channel has a name, a topic and teams.
const MAX_PARTICIPANTS: usize = 8;

#[derive(serde::Serialize)]
struct DmEnvelope {
    channel: talaria_channels::CreatedChannel,
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;

    if !obj.contains_key("userIds") && !obj.contains_key("agents") {
        let other = match uuid_member(obj, "userId") {
            Ok(u) => u,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        };
        // ensure_dm's own user-facing message (DM with yourself, unknown user)
        // answers as a 400.
        return Ok(match ensure_dm(&state.pg, &user.id, &other).await {
            Ok(channel) => (StatusCode::OK, Json(DmEnvelope { channel })).into_response(),
            Err(msg) => house_error(StatusCode::BAD_REQUEST, &msg),
        });
    }

    let user_ids = match optional_string_array_member(obj, "userIds", 36, 36, MAX_PARTICIPANTS) {
        Ok(v) => v.unwrap_or_default(),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if user_ids.iter().any(|u| !talaria_body::zod_uuid_ok(u)) {
        return Ok(house_error(StatusCode::BAD_REQUEST, "Invalid UUID"));
    }
    let agents = match optional_string_array_member(obj, "agents", 1, 200, MAX_PARTICIPANTS) {
        Ok(v) => v.unwrap_or_default(),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let name = match optional_max_string_member(obj, "name", 80) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    // Everyone named must be a real person here — a typo'd id is a 400, not a
    // foreign-key error leaking out of the insert.
    let others: Vec<&String> = user_ids.iter().filter(|u| **u != user.id).collect();
    if !others.is_empty() {
        let known: i64 =
            match sqlx::query_scalar("select count(*) from users where id = any($1::uuid[])")
                .bind(others.iter().map(|s| s.as_str()).collect::<Vec<_>>())
                .fetch_one(&state.pg)
                .await
            {
                Ok(n) => n,
                Err(e) => return Ok(internal("[dms] member read failed", e)),
            };
        let mut distinct = others.clone();
        distinct.sort();
        distinct.dedup();
        if known != distinct.len() as i64 {
            return Ok(house_error(StatusCode::BAD_REQUEST, "unknown user"));
        }
    }
    // Agents: the same gate as seating one in a channel — you may only bring
    // an agent you can use (a personal assistant only by its owner).
    if !agents.is_empty() {
        let gate = match usable_agent_gate(&state.pg, &user.id, &user.role).await {
            Ok(g) => g,
            Err(e) => return Ok(internal("[dms] agent access read failed", e)),
        };
        if let Some(bad) = agents.iter().find(|m| !gate(m)) {
            return Ok(house_error(
                StatusCode::FORBIDDEN,
                &format!("forbidden: no access to agent {bad}"),
            ));
        }
    }

    Ok(
        match ensure_group_dm(&state.pg, &user.id, &user_ids, &agents, name.as_deref()).await {
            Ok(channel) => (StatusCode::OK, Json(DmEnvelope { channel })).into_response(),
            Err(msg) => house_error(StatusCode::BAD_REQUEST, &msg),
        },
    )
}
