// /api/agent/whoami. GET (agent key) → who is calling and what it may touch.
//
// THE ROUTE AN AGENT PROBES WITH. Before this, the only way an agent could
// learn its own reach was to try a verb and read the 403 — Gregosaurus's
// audit: `list_boards` shows the boards, `list_tickets` on them refuses, and
// nothing in between says which. This answers the question in one cheap GET:
// identity (proven? personal? whose? elevated?), the boards listing with a
// WHY per row (the same merge `GET /api/boards` serves the agent branch, each
// row tagged with the arm that produced it), a channel/server summary, the
// guardrails that will refuse it, and its own still-open access requests.
//
// It is also the fleet verifier's durable probe: an agent-credential GET that
// answers 200 for a good key and 401 for a bad one, never 404/405. That
// contract is written at both ends (mcp/src/index.ts `verify`, mcp/README.md
// § "Authentication — and the route it depends on"): narrowing this route —
// admin-only, session-only, moved, renamed — takes the fleet toolkit dark
// fleet-wide. (The oracle lived on GET /api/users before this route existed;
// that borrow was one permissions decision away from an outage.)
//
// READ-ONLY, EVERY CLAIM COMPOSED. Nothing here grants, decides or mutates;
// every field is an existing resolver's answer (agent_caller,
// assistant_owner_for, is_elevated_assistant, the three board listings,
// list_channels_for_agent, servers_for_agent), so whoami can never become a
// second opinion about access — it reports what the routes would do.

use crate::agent_auth::{AgentSubject, epoch_ms_to_iso, require_agent, subject_proven};
use crate::boards::{list_all_boards, list_boards, list_boards_for_agent};
use crate::channels::list_channels_for_agent;
use crate::mcp::registry::servers_for_agent;
use crate::users::{assistant_owner_for, is_elevated_assistant};
use axum::extract::State;
use serde_json::{Value, json};

/// The refusal sentences an agent will actually receive, stated up front
/// instead of discovered verb by verb. Mirrored from the enforcing code
/// (boards_id_tasks, agent_safe_patch, agent_ticket_refusal) — if those
/// change, this list follows or it lies.
const GUARDRAILS: [&str; 5] = [
    "agents cannot assign tickets — assignment is a human call",
    "agents cannot take a ticket out of review — a person signs it off",
    "agents cannot change a closed ticket",
    "agents cannot work an archived ticket — a person restores it first",
    "deleting boards, tickets or members is a human-only route",
];

pub async fn get(
    State(state): State<crate::state::AppState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let subject = AgentSubject::Caller(caller.clone());
    let model = caller.model.as_str();

    // Display name from the registry — the label the workspace knows.
    let name: Option<(String,)> = sqlx::query_as(
        "select display_name from agent_defs where model = $1",
    )
    .bind(model)
    .fetch_optional(&state.pg)
    .await
    .unwrap_or(None);

    let owner = match assistant_owner_for(&state.pg, &subject).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[agent.whoami] owner lookup failed: {e}");
            return crate::error::thrown_internal_error();
        }
    };
    let owner_json = match &owner {
        Some(owner_id) => {
            let label: Option<(Option<String>, Option<String>)> = sqlx::query_as(
                "select email, name from users where id = $1::uuid",
            )
            .bind(owner_id)
            .fetch_optional(&state.pg)
            .await
            .unwrap_or(None);
            json!({
                "id": owner_id,
                "label": label
                    .and_then(|(email, name)| email.or(name))
                    .unwrap_or_else(|| owner_id.clone()),
            })
        }
        None => Value::Null,
    };
    let elevated = match is_elevated_assistant(&state.pg, &subject).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[agent.whoami] elevation read failed: {e}");
            return crate::error::thrown_internal_error();
        }
    };

    // The boards answer mirrors GET /api/boards's agent branch arm for arm,
    // each row tagged with the arm that produced it: "owner" (the owner's
    // boards, under the owner's role), "policy" (the board's agent policy),
    // "elevated" (org-wide, answered as editor). Same dedupe, same order —
    // owner first, so a board both arms cover reports the stronger why.
    let policy_boards = match list_boards_for_agent(&state.pg, model).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[agent.whoami] agent board listing failed: {e}");
            return crate::error::thrown_internal_error();
        }
    };
    let mut boards: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = Default::default();
    if let Some(owner_id) = &owner {
        let owner_boards = match list_boards(&state.pg, owner_id, false).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[agent.whoami] owner board listing failed: {e}");
                return crate::error::thrown_internal_error();
            }
        };
        for b in owner_boards {
            seen.insert(b.id.clone());
            boards.push(json!({
                "id": b.id,
                "name": b.name,
                "why": "owner",
                "role": b.role,
            }));
        }
    }
    let rest = if elevated {
        match list_all_boards(&state.pg).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[agent.whoami] org-wide board listing failed: {e}");
                return crate::error::thrown_internal_error();
            }
        }
    } else {
        policy_boards
    };
    for b in rest {
        if !seen.insert(b.id.clone()) {
            continue;
        }
        boards.push(json!({
            "id": b.id,
            "name": b.name,
            "why": if elevated { "elevated" } else { "policy" },
            "role": if elevated { json!("editor") } else { Value::Null },
        }));
    }

    // A summary, not the channels payload itself — the listing routes stay
    // the source of truth; this answers "do I have anywhere to speak".
    let channels = match list_channels_for_agent(&state.pg, &subject).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[agent.whoami] channel listing failed: {e}");
            return crate::error::thrown_internal_error();
        }
    };
    let servers = match servers_for_agent(&state.pg, model).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[agent.whoami] server listing failed: {e}");
            return crate::error::thrown_internal_error();
        }
    };

    // Its own open requests — the pending half of the ask, so an agent that
    // filed one stops re-filing and can tell its owner where things stand.
    let requests: Vec<(String, String, String, i64)> = match sqlx::query_as(
        "select r.board_id::text, b.name, r.status, \
                (trunc(extract(epoch from r.created_at) * 1000))::bigint \
         from board_agent_requests r join boards b on b.id = r.board_id \
         where r.agent_model = $1 and r.status = 'open' \
         order by r.created_at",
    )
    .bind(model)
    .fetch_all(&state.pg)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[agent.whoami] request listing failed: {e}");
            return crate::error::thrown_internal_error();
        }
    };

    axum::Json(json!({
        "agent": {
            "model": model,
            "name": name.map(|(n,)| n),
            // The agent_defs row this credential proved — null for a legacy
            // shared-key caller, which is identified but not proven.
            "id": caller.id,
            "proven": subject_proven(&subject),
            "legacy": caller.legacy,
            "personal": owner.is_some(),
            "owner": owner_json,
            "elevated": elevated,
        },
        "boards": boards,
        "channels": channels.iter().map(|c| json!({
            "id": c.id,
            "name": c.name,
            "kind": c.kind,
        })).collect::<Vec<_>>(),
        "servers": servers.iter().map(|s| &s.name).collect::<Vec<_>>(),
        "pendingRequests": requests.iter().map(|(board_id, board_name, status, ms)| json!({
            "boardId": board_id,
            "boardName": board_name,
            "status": status,
            "createdAt": epoch_ms_to_iso(*ms),
        })).collect::<Vec<_>>(),
        "guardrails": GUARDRAILS,
        "selfService": {
            "join": "POST /api/boards/{boardId}/agents/self",
            "leave": "DELETE /api/boards/{boardId}/agents/self",
            "request": "POST /api/boards/{boardId}/agent-requests",
            "note": "join needs the agent's own credential and a board its owner can read; \
                     request is the path when the owner cannot",
        },
    }))
    .into_response()
}
