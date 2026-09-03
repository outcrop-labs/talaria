// GET /api/users. Everyone who has signed in (id, email, name), for the
// people pickers. Any signed-in user — and agents (their own tak_ key or the
// fleet key): they need the directory to resolve "email Priya" or "add Priya
// to the board" into an address.
//
// This route once doubled as mcp/'s fleet-wide auth oracle (the resident GET
// /api/users with the agent's credential to check the fleet was still talking
// to its org). The oracle has its own home now — GET /api/agent/whoami, see
// routes/agents/agent_whoami.rs — and narrowing THIS route to a people-picker
// decision no longer takes the toolkit dark.

use crate::agent_auth::agent_caller;
use crate::error::thrown_internal_error;
use crate::session::require_user;
use crate::state::AppState;
use crate::users::list_users;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

#[derive(serde::Serialize)]
struct DirectoryUser {
    id: String,
    email: Option<String>,
    name: Option<String>,
}

#[derive(serde::Serialize)]
struct UsersBody {
    users: Vec<DirectoryUser>,
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // A presented agent credential that is REJECTED returns its refusal —
    // falling through would turn a forgery into a quiet 401.
    match agent_caller(&state.pg, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            if let Err(gate) = require_user(&state, &headers).await {
                return gate;
            }
        }
        Err(resp) => return resp,
    }
    match list_users(&state.pg).await {
        Ok(rows) => Json(UsersBody {
            users: rows
                .into_iter()
                .map(|(id, email, name)| DirectoryUser { id, email, name })
                .collect(),
        })
        .into_response(),
        Err(e) => {
            tracing::error!("[users] directory query failed: {e}");
            thrown_internal_error()
        }
    }
}
