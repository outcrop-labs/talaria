// POST /api/chat/chips/approvals/{id} — approve or deny a chip.
// Google sends execute through the existing confirm-send. Ticket moves (a
// refused `triage_ticket` → `ticket_move`, a refused `move_ticket` →
// `ticket_board_move`) apply as the person who approved, and are THE BOARD'S
// to decide: any editor of the ticket's board, or an admin — not the agent's
// owner, who may not even see the board. That grants nothing new, because an
// editor can make the move on the board themselves. Every other kind's chip
// names the tools its approval unlocked. An agent's own approval request
// (kind `hermes_approval`) is answered through the agent instead: `once` or
// `deny`, by the conversation's owner while they manage the agent.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::now_ms;
use talaria_api_facades::fleet::describe_agent;
use talaria_api_facades::google::pending_actions::decide_action;
use talaria_boards::{board_role, can_edit};
use talaria_body::{enum_member, parse};
use talaria_chips::{
    is_google_action, is_ticket_approval, load_approval, mark_approval, merge_mcp_unlock,
    merge_unlocked, surface_for_agent, tools_unlocked, unlock_chip_for,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::secretbox_or_500;
use talaria_session::{SessionUser, actor_of, require_user};
use talaria_state::AppState;
use talaria_tasks::{
    MoveError, MoveRequest, Mover, ResolvedTaskId, TaskActor, TaskDeps, TaskError, TaskPatch,
    get_task, move_task, resolve_task_id, update_task,
};

// doc: Approve or deny a protected action surfaced as a chip in the thread.

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let decision = match enum_member(obj, "decision", &["approve", "reject"]) {
        Ok(d) => d,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    let row = match load_approval(&state.pg, &id).await {
        Ok(Some(row)) => row,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[chips] approval read failed", e)),
    };
    if row.status != "pending" {
        return Ok(Json(json!({ "status": row.status, "tools": [] })).into_response());
    }
    // An agent's own approval request, put to the person whose conversation
    // it is: answered through the agent's door, not executed here.
    if row.kind == talaria_approval_cards::KIND {
        return Ok(answer_card(&state, &user, &row, &decision).await);
    }
    let owner_or_admin =
        row.owner_user_id.as_deref() == Some(user.id.as_str()) || user.role == "admin";
    // Saying NO to a ticket card grants nothing, so whoever could always
    // answer a card (its owner, an admin) may still decline it without
    // editing the board. Only an approval runs a move, and that is the
    // board's to gate.
    if decision == "reject" && is_ticket_approval(&row.kind) && owner_or_admin {
        if let Err(e) = mark_approval(&state.pg, &row.id, "denied", &user.id).await {
            return Ok(internal("[chips] deny failed", e));
        }
        return Ok(Json(json!({ "status": "denied", "tools": [] })).into_response());
    }
    // A ticket move is gated on the ticket's board, and its payload's ticket
    // (a ref or an id) is resolved here, once, for the gate and the move.
    let ticket = if is_ticket_approval(&row.kind) {
        match ticket_gate(&state, &user, &row).await {
            Ok(id) => Some(id),
            Err(response) => return Ok(response),
        }
    } else {
        if !owner_or_admin {
            return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
        }
        None
    };
    if decision == "reject" {
        if let Err(e) = mark_approval(&state.pg, &row.id, "denied", &user.id).await {
            return Ok(internal("[chips] deny failed", e));
        }
        return Ok(Json(json!({ "status": "denied", "tools": [] })).into_response());
    }

    if let Err(response) = execute(&state, &user, &row, ticket.as_deref()).await {
        return Ok(response);
    }
    if let Err(e) = mark_approval(&state.pg, &row.id, "approved", &user.id).await {
        return Ok(internal("[chips] approve mark failed", e));
    }
    // What the approval unlocks for the rest of the conversation. A held
    // Google Workspace MCP call (`mcp_call`) earns a grant keyed by its own
    // server and tool, and only when it ran for a personal principal (D7:
    // an org-account or agent-identity call needs an admin every time).
    // Every other kind unlocks its static tools.
    let tools: Vec<String> = if row.kind == "mcp_call" {
        match (&row.conversation_id, row.external_id.as_deref()) {
            (Some(conversation_id), Some(external)) => {
                match merge_mcp_unlock(&state.pg, conversation_id, external).await {
                    Ok(key) => key.into_iter().collect(),
                    Err(e) => return Ok(internal("[chips] unlock failed", e)),
                }
            }
            _ => Vec::new(),
        }
    } else {
        let tools = tools_unlocked(&row.kind);
        if let Some(conversation_id) = &row.conversation_id
            && let Err(e) = merge_unlocked(&state.pg, conversation_id, tools).await
        {
            return Ok(internal("[chips] unlock failed", e));
        }
        tools.iter().map(|t| t.to_string()).collect()
    };
    // An approval that earned no grant — an `mcp_call` for an org or agent
    // principal, or a ticket move, which IS the move — shows no unlock at
    // all; every other kind keeps its chip as before.
    if let Some(agent) = &row.agent_model
        && !tools.is_empty()
    {
        let chip = unlock_chip_for(&row.id, &row.kind, &tools);
        let redis = state.redis().await.ok();
        if let Err(e) = surface_for_agent(&state.pg, redis, agent, vec![chip]).await {
            return Ok(internal("[chips] unlock chip failed", e));
        }
    }
    Ok(Json(json!({ "status": "approved", "tools": tools })).into_response())
}

/// `approve` is `once` and `reject` is `deny`, sent as
/// `POST /v1/runs/{run_id}/approval {request_id, execution_generation, choice}`
/// on the agent's own door. Who may answer is the card's rule
/// (talaria-approval-cards), checked again here: the conversation's owner,
/// and only while they manage the agent. A card the agent stopped waiting on
/// answers 409 and reads expired.
async fn answer_card(
    state: &AppState,
    user: &talaria_session::SessionUser,
    row: &talaria_chips::ApprovalRow,
    decision: &str,
) -> Response {
    use talaria_approval_cards::{Choice, Decided, Person};
    let choice = if decision == "approve" {
        Choice::Once
    } else {
        Choice::Deny
    };
    let actor = actor_of(user);
    let person = Person {
        user_id: &user.id,
        role: &user.role,
        actor: &actor,
    };
    let redis = state.redis().await.ok();
    match talaria_approval_cards::decide(&state.pg, redis, row, &person, choice).await {
        Decided::Answered(status) => Json(json!({ "status": status, "tools": [] })).into_response(),
        Decided::Settled(status) => Json(json!({ "status": status, "tools": [] })).into_response(),
        Decided::Forbidden => house_error(StatusCode::FORBIDDEN, "forbidden"),
        Decided::NotOffered => house_error(
            StatusCode::BAD_REQUEST,
            "the agent cannot take that answer for this request",
        ),
        Decided::Expired => house_error(
            StatusCode::CONFLICT,
            "the agent was no longer waiting for this answer",
        ),
        Decided::Undelivered(why) => {
            tracing::warn!(
                "[chips] approval {}: the answer did not reach the agent: {why}",
                row.id
            );
            house_error(
                StatusCode::BAD_GATEWAY,
                "the answer did not reach the agent — try again",
            )
        }
        Decided::Db(e) => internal("[chips] approval answer failed", e),
    }
}

/// Who may decide a ticket-move card: an editor of the ticket's board, or an
/// admin. Answers the ticket's id, resolved from the payload's `taskId` —
/// a ref (`ABC-12`, an old ref included) or an id, as the agent sent it.
async fn ticket_gate(
    state: &AppState,
    user: &SessionUser,
    row: &talaria_chips::ApprovalRow,
) -> Result<String, Response> {
    let raw = row
        .payload
        .get("taskId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if raw.is_empty() {
        return Err(house_error(
            StatusCode::BAD_REQUEST,
            "no ticket on this approval",
        ));
    }
    let id = match resolve_task_id(&state.pg, raw).await {
        Ok(ResolvedTaskId::One(id)) => id,
        Ok(ResolvedTaskId::Ambiguous) => {
            return Err(house_error(
                StatusCode::BAD_REQUEST,
                &format!("\"{raw}\" names more than one ticket — move it from the board instead"),
            ));
        }
        Ok(ResolvedTaskId::Missing | ResolvedTaskId::Malformed) => {
            return Err(house_error(StatusCode::NOT_FOUND, "not found"));
        }
        Err(e) => return Err(internal("[chips] ticket resolve failed", e)),
    };
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return Err(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Err(internal("[chips] ticket read failed", e)),
    };
    if user.role == "admin" {
        return Ok(id);
    }
    match board_role(&state.pg, &user.id, &task.board_id).await {
        Ok(role) if can_edit(role.as_deref()) => Ok(id),
        Ok(_) => Err(house_error(
            StatusCode::FORBIDDEN,
            "only someone who can edit this ticket's board can approve moving it",
        )),
        Err(e) => Err(internal("[chips] board role read failed", e)),
    }
}

async fn execute(
    state: &AppState,
    user: &SessionUser,
    row: &talaria_chips::ApprovalRow,
    ticket: Option<&str>,
) -> Result<(), Response> {
    let user_id = user.id.as_str();
    let is_admin = user.role == "admin";
    // EVERY Google kind routes through the confirm-send plane, and the test
    // is `is_google_action` rather than a second hand-written list of kinds.
    // That is the bug this shape exists to prevent: the arm used to name
    // `gmail_send` and `calendar_create` alone, so a queued Doc edit, Drive
    // move or rename, or calendar change fell through to the catch-all and
    // was marked APPROVED WITHOUT EVER BEING SENT — the person saw the card
    // clear and Google never heard about it.
    if is_google_action(&row.kind) {
        let external = row.external_id.as_deref().unwrap_or("");
        if external.is_empty() {
            return Err(house_error(StatusCode::BAD_REQUEST, "nothing to send"));
        }
        let sb = secretbox_or_500(state, "[chips] confirm-send").await?;
        return match decide_action(
            &state.pg,
            &sb,
            external,
            user_id,
            is_admin,
            "approve",
            now_ms(),
        )
        .await
        {
            Ok(Some(outcome)) if outcome.status == "executed" => Ok(()),
            Ok(Some(outcome)) if outcome.status == "forbidden" => {
                Err(house_error(StatusCode::FORBIDDEN, "forbidden"))
            }
            Ok(Some(outcome)) => Err(house_error(
                StatusCode::BAD_GATEWAY,
                outcome
                    .message
                    .as_deref()
                    .unwrap_or("the action did not send"),
            )),
            Ok(None) => Err(house_error(StatusCode::NOT_FOUND, "not found")),
            Err(e) => Err(internal("[chips] confirm-send failed", e)),
        };
    }

    let text = |key: &str| {
        row.payload
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    match row.kind.as_str() {
        // The move runs as the PERSON who approved — their email on the
        // activity line like every human actor, never a uuid — and the line
        // says whose request it was.
        "ticket_move" => {
            let Some(task_id) = ticket else {
                return Err(house_error(
                    StatusCode::BAD_REQUEST,
                    "no ticket on this approval",
                ));
            };
            let agent = row
                .agent_model
                .as_deref()
                .map(|m| describe_agent(m).label)
                .unwrap_or_else(|| "the agent".into());
            let patch = TaskPatch {
                title: text("title"),
                status: text("status"),
                priority: text("priority"),
                description: text("description").map(Some),
                status_note: Some(format!("approved {agent}'s request")),
                ..TaskPatch::default()
            };
            let deps = TaskDeps::from_route(state.pg.clone(), state.redis().await.ok());
            match update_task(&deps, task_id, patch, &TaskActor::human(actor_of(user))).await {
                Ok(_) => Ok(()),
                Err(TaskError::ApprovalRequired(msg) | TaskError::Refusal(msg)) => {
                    Err(house_error(StatusCode::BAD_REQUEST, &msg))
                }
                Err(TaskError::Db(e)) => Err(internal("[chips] ticket move failed", e)),
            }
        }
        // The board move the agent was refused, made by the person who
        // approved it: `move_task` checks THEIR edit right on both boards.
        "ticket_board_move" => {
            let (Some(task_id), Some(board_id)) = (ticket, text("boardId")) else {
                return Err(house_error(
                    StatusCode::BAD_REQUEST,
                    "no ticket or board on this approval",
                ));
            };
            if talaria_params::uuid_canonical(&board_id).is_none() {
                return Err(house_error(StatusCode::BAD_REQUEST, "no such board"));
            }
            let status = text("status");
            let req = MoveRequest {
                board_id: &board_id,
                status: status.as_deref(),
                preview: false,
                ask: None,
            };
            let mover = Mover::Human {
                user_id: user.id.clone(),
                actor: actor_of(user),
            };
            let deps = TaskDeps::from_route(state.pg.clone(), state.redis().await.ok());
            match move_task(&deps, task_id, &req, &mover).await {
                Ok(_) => Ok(()),
                Err(MoveError::NotFound) => Err(house_error(StatusCode::NOT_FOUND, "not found")),
                Err(MoveError::Bad(msg)) => Err(house_error(StatusCode::BAD_REQUEST, &msg)),
                Err(MoveError::Forbidden(msg)) => Err(house_error(StatusCode::FORBIDDEN, &msg)),
                Err(MoveError::Conflict(msg)) => Err(house_error(StatusCode::CONFLICT, &msg)),
                Err(MoveError::Db(e)) => Err(internal("[chips] board move failed", e)),
            }
        }
        // The approval IS the grant — there is nothing else to run.
        "board_access" => Ok(()),
        // Not a silent success. An approval whose kind nothing here executes
        // would otherwise be marked approved and quietly do nothing, which is
        // exactly how the Google kinds went missing.
        other => {
            tracing::error!("[chips] approval kind '{other}' has no execution path");
            Err(house_error(
                StatusCode::BAD_REQUEST,
                "this approval has no action behind it",
            ))
        }
    }
}
