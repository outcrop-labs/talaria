// /api/boards — port of ui/src/routes/api/boards.ts. GET → the boards the
// caller owns or that are shared with them; an agent key swaps the question
// for the boards whose POLICY allows that agent, plus — for a personal
// assistant — its owner's boards under the owner's role (the identity-proxy
// model, so it can govern them on the owner's behalf). POST { name } →
// create a board, the caller becoming its owner.

use crate::agent_auth::{AgentSubject, agent_caller};
use crate::boards::{
    AgentBoard, Board, create_board, list_all_boards, list_boards, list_boards_for_agent,
};
use crate::body::{as_object, optional_uuid_member, parse, string_member};
use crate::error::house_error;
use crate::permissions::has_perm;
use crate::session::require_user;
use crate::state::AppState;
use crate::teams::team_role;
use crate::users::{assistant_owner_for, is_elevated_assistant};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// The elevated-assistant listing row: every live board org-wide, answered
/// AS an editor (never owner-level) — TS's `{ ...b, role: 'editor' }`.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentBoardAsEditor<'a> {
    #[serde(flatten)]
    board: &'a AgentBoard,
    role: &'static str,
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    // The dual-auth question: an agent credential, else a session. Err is the
    // refusal to return verbatim; Ok(None) means no credential was presented
    // and the human path takes over.
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(Some(c)) => c,
        Ok(None) => return get_as_user(&state, &headers, &uri).await,
        Err(resp) => return resp,
    };
    let subject = AgentSubject::Caller(caller.clone());
    let policy_boards = match list_boards_for_agent(&state.pg, &caller.model).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[boards] agent listing failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    // Owner-proxying and org-wide reach key off the CALLER: a legacy
    // shared-key caller only ever gets the boards its policy allows.
    let owner_id = match assistant_owner_for(&state.pg, &subject).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[boards] owner lookup failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let Some(owner_id) = owner_id else {
        return Json(json!({ "boards": policy_boards })).into_response();
    };
    let owner_boards = match list_boards(&state.pg, &owner_id, false).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[boards] owner listing failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let elevated = match is_elevated_assistant(&state.pg, &subject).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[boards] elevation read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    // The merged listing is heterogeneous BY DESIGN: the owner's boards carry
    // their role, the elevated rest carries 'editor', and a plain agent's
    // rest carries no role at all — three wire shapes TS emits from three
    // sources, serialized per-row here rather than flattened into one struct
    // that would invent a role for the third.
    let seen: std::collections::HashSet<&str> =
        owner_boards.iter().map(|b| b.id.as_str()).collect();
    let mut boards: Vec<Value> = owner_boards
        .iter()
        .map(|b| serde_json::to_value(b).unwrap_or(Value::Null))
        .collect();
    let rest: Vec<AgentBoard> = if elevated {
        match list_all_boards(&state.pg).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[boards] org-wide listing failed: {e}");
                return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
            }
        }
    } else {
        policy_boards
    };
    for b in rest {
        if seen.contains(b.id.as_str()) {
            continue;
        }
        let v = if elevated {
            serde_json::to_value(AgentBoardAsEditor {
                board: &b,
                role: "editor",
            })
        } else {
            serde_json::to_value(&b)
        };
        boards.push(v.unwrap_or(Value::Null));
    }
    Json(json!({ "boards": boards })).into_response()
}

async fn get_as_user(state: &AppState, headers: &HeaderMap, uri: &Uri) -> Response {
    let user = match require_user(state, headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // searchParams.get('archived') === '1' — the retired boards are asked for
    // by name; everything else sees the live ones.
    let archived = uri
        .query()
        .and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find(|(k, _)| k == "archived")
                .map(|(_, v)| v.into_owned())
        })
        .as_deref()
        == Some("1");
    let boards: Vec<Board> = match list_boards(&state.pg, &user.id, archived).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[boards] list failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    Json(json!({ "boards": boards })).into_response()
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // Authorization BEFORE body parsing — never do work for a caller who
    // can't take the action.
    let allowed = match has_perm(&state.pg, &user.id, &user.role, "boards.create").await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[boards] permission read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    if !allowed {
        return house_error(StatusCode::FORBIDDEN, "no permission to create boards");
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match string_member(obj, "name", 1, 120) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let team_id = match optional_uuid_member(obj, "teamId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Team boards require membership in that team.
    if let Some(tid) = &team_id {
        match team_role(&state.pg, &user.id, tid).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return house_error(StatusCode::FORBIDDEN, "not a member of that team");
            }
            Err(e) => {
                tracing::error!("[boards] team role read failed: {e}");
                return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
            }
        }
    }
    let board = match create_board(&state.pg, &user.id, &name, team_id.as_deref()).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("[boards] create failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    Json(json!({ "board": board })).into_response()
}
