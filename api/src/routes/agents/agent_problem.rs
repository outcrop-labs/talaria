// /api/agent/problem. POST (agent key) → an agent hit something broken it
// shouldn't explain to a normal person. Talaria elevates it: the admins who
// may hear it get an alert notification, a Helpdesk ticket carries the
// technical details (board find-or-created), and the agent gets plain-language
// confirmation to relay.
//
// THE ROUTE LEARNS WHICH TICKET THE AGENT WAS ON. `taskId` arrives from the
// agent and is AUTHORISED exactly the way /api/agent/gap authorises its own —
// unknown and not-allowed refuse identically (a distinct 404 is a ticket
// enumeration oracle), and a ticket a person has taken off the table refuses
// too. Then the authority is `agentTextAuthority`, the same answer the
// capability gap gets, and the audience is `audienceFor` like everything
// else.
//
// WHAT EACH HALF IS TOLD. `content` — the admins who can see that board, or
// every admin when the problem is genuinely org-wide — gets the summary, the
// context and the link. `fact` gets the title without the summary, no body
// text the agent wrote, and NO LINK, because the link is to a ticket
// carrying the details. They are told an agent is blocked, which is the
// thing they can act on: grant the access, fix the credential, add
// themselves to the board.
//
// KNOWN RESIDUAL, stated rather than hidden: the Helpdesk ticket itself is
// not scoped by the originating board, so anyone who can see the Helpdesk
// board — which is the whole workspace, by design — can read the details
// there. That is a PULL surface someone chose to open, not a push into an
// inbox and a mailbox.

use crate::agent_auth::{AgentSubject, require_agent};
use crate::approvals::audience_for;
use crate::boards::{
    create_board, join_everyone_to_board, list_all_boards, set_board_agent_config,
};
use crate::body::{
    as_object, optional_max_string_member, optional_uuid_member, parse, string_member,
};
use crate::error::{house_error, thrown_internal_error};
use crate::fleet::describe_agent;
use crate::gaps::{agent_text_authority, remember_ticket_refusal};
use crate::notify::{NotificationInput, NotifyDeps, add_notification};
use crate::runs::define::Authority;
use crate::state::AppState;
use crate::tasks::{
    AgentIntent, AgentWriteTarget, NewTask, TaskDeps, agent_ticket_refusal, create_task, get_task,
};
use crate::teams::create_team;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

const HELPDESK: &str = "Helpdesk";
const TALARIA: &str = "Talaria";

/// The workspace Helpdesk — "Talaria > Helpdesk", org-wide. Tickets agents
/// file about their own gaps land on ONE board the whole workspace can open,
/// not a private board owned by whichever admin happened to be earliest.
///
/// Every call runs the full idempotent ensure: claim the board for the
/// workspace (org_wide + under the Talaria team, so the nav groups it under
/// "Talaria"), join everyone who exists, and keep it open to every agent.
/// That last part is also the UPGRADE path — a Helpdesk board created by the
/// old code is found by name and claimed rather than left personal.
/// Any failure inside reads as "no board".
async fn helpdesk_board(pg: &sqlx::PgPool) -> Option<String> {
    let admin: Option<(String,)> = sqlx::query_as(
        "select id::text from users order by (role = 'admin') desc, created_at asc limit 1",
    )
    .fetch_optional(pg)
    .await
    .ok()
    .flatten();
    let (admin_id,) = admin?;
    // The Talaria team is the system's shelf — the group header these boards
    // sit under in the nav. No unique constraint on teams.name, so
    // find-then-create.
    let team: Option<(String,)> =
        sqlx::query_as("select id::text from teams where name = $1 limit 1")
            .bind(TALARIA)
            .fetch_optional(pg)
            .await
            .ok()
            .flatten();
    let team_id = match team {
        Some((id,)) => id,
        None => create_team(pg, &admin_id, TALARIA).await.ok()?.0,
    };
    let existing = list_all_boards(pg)
        .await
        .ok()?
        .into_iter()
        .find(|b| b.name.to_lowercase() == HELPDESK.to_lowercase());
    let board_id = match existing {
        Some(b) => b.id,
        None => {
            create_board(pg, &admin_id, HELPDESK, Some(&team_id))
                .await
                .ok()?
                .id
        }
    };
    sqlx::query("update boards set org_wide = true, team_id = $2::uuid where id = $1::uuid")
        .bind(&board_id)
        .bind(&team_id)
        .execute(pg)
        .await
        .ok()?;
    join_everyone_to_board(pg, &board_id).await.ok()?;
    set_board_agent_config(pg, &board_id, true, &[])
        .await
        .ok()?;
    Some(board_id)
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let agent = caller.model.clone();
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let summary = match string_member(obj, "summary", 5, 300) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let details = match optional_max_string_member(obj, "details", 20_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // what the agent was trying to do
    let context = match optional_max_string_member(obj, "context", 500) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // the ticket the agent was working when it broke
    let task_id = match optional_uuid_member(obj, "taskId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let label = describe_agent(&agent).label;

    // NEVER tell a refused agent to drop `taskId` and retry, and do not
    // leave the retry working in silence either: each refusal is remembered
    // against the caller and `agentTextAuthority` reads that memo below.
    let task = match task_id.as_deref() {
        Some(id) => match get_task(&state.pg, id).await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("[agent.problem] ticket read failed: {e}");
                return thrown_internal_error();
            }
        },
        None => None,
    };
    if let Some(task_id) = task_id.as_deref() {
        let refuse = || {
            house_error(
                StatusCode::FORBIDDEN,
                &format!(
                    "taskId \"{task_id}\" is not a ticket you may write to. Ask for access to its \
                     board, then report the problem against the ticket."
                ),
            )
        };
        let Some(task) = task.as_ref() else {
            remember_ticket_refusal(&state.pg, &agent, None).await;
            return refuse();
        };
        // The CALLER, not its model: board policy's elevated bypass is only
        // for an identity that was proven, never merely asserted.
        let allowed = match crate::boards::board_allows_agent(
            &state.pg,
            &task.board_id,
            &AgentSubject::Caller(caller.clone()),
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[agent.problem] board policy read failed: {e}");
                return thrown_internal_error();
            }
        };
        if !allowed {
            remember_ticket_refusal(&state.pg, &agent, None).await;
            return refuse();
        }
        let target = AgentWriteTarget {
            board_id: task.board_id.clone(),
            status: task.status.clone(),
            archived_at: task.archived_at.clone(),
        };
        match agent_ticket_refusal(
            &state.pg,
            &target,
            &AgentSubject::Caller(caller.clone()),
            AgentIntent::Write,
        )
        .await
        {
            Ok(None) => {}
            Ok(Some(shut)) => {
                remember_ticket_refusal(&state.pg, &agent, Some(&task.board_id)).await;
                let mut resp =
                    Json(json!({ "error": "forbidden", "message": shut })).into_response();
                *resp.status_mut() = StatusCode::FORBIDDEN;
                return resp;
            }
            Err(e) => {
                tracing::error!("[agent.problem] ticket refusal read failed: {e}");
                return thrown_internal_error();
            }
        }
    }

    // Ticket first (it carries the technical substance).
    let mut ticket_note = "a Helpdesk ticket could not be filed";
    let mut href = "/observability/alerts".to_string();
    if let Some(board_id) = helpdesk_board(&state.pg).await {
        let task = task.as_ref();
        let description = format!(
            "**Reported by agent:** {label} ({agent})\n\n{}{}**Technical details:**\n\n{}",
            context
                .as_deref()
                .map(|c| format!("**While:** {c}\n\n"))
                .unwrap_or_default(),
            task.map(|t| {
                format!(
                    "**On ticket:** [{}](/boards/{}/{})\n\n",
                    t.title, t.board_id, t.id
                )
            })
            .unwrap_or_default(),
            details.as_deref().unwrap_or("(none provided)"),
        );
        let deps = TaskDeps::from_route(state.pg.clone(), state.redis().await.ok());
        let filed = create_task(
            &deps,
            &NewTask {
                board_id: &board_id,
                title: &format!("[{label}] {summary}"),
                description: Some(&description),
                priority: Some("high"),
                effort: None,
                assignees: &[],
                due_date: None,
                start_date: None,
                color: None,
                estimated_hours: None,
                parent_id: None,
                tags: &[],
                created_by: &agent,
            },
        )
        .await;
        match filed {
            Ok(filed) => {
                ticket_note = "Helpdesk ticket filed";
                href = format!("/boards/{board_id}/{}", filed.id);
            }
            Err(e) => {
                tracing::error!("[agent.problem] helpdesk filing failed: {}", e.message());
                return thrown_internal_error();
            }
        }
    }

    // ONE announcement path, and it asks the resolver.
    let authority = agent_text_authority(
        &state.pg,
        &agent,
        task.as_ref().map(|t| t.board_id.as_str()),
    )
    .await;
    let who = audience_for(&state.pg, &authority).await;
    // WHY the fact recipient is not being told, and it is two different
    // reasons. `{ by: 'admin', onBoard }` withheld the words because the
    // reader is not on the board they quote. `{ by: 'nobody' }` withheld
    // them because the agent was refused a ticket and retried without one,
    // so this is not an org-wide report and there is no board to name — and
    // in THAT case the reader may well be a member of the board the text is
    // actually about. Printing the first sentence for both told an on-board
    // admin they were not a member of work they are a member of, which is
    // the announcement lying about its own reason.
    let placed = matches!(authority, Authority::Admin { on_board: Some(_) });
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    for user_id in &who.content {
        if let Err(e) = add_notification(
            &notify,
            user_id,
            &NotificationInput {
                kind: "agent-problem",
                title: &format!("{label} hit a problem: {summary}"),
                body: Some(context.as_deref().unwrap_or("")),
                href: Some(&href),
            },
        )
        .await
        {
            tracing::error!("[agent-problem] could not notify {user_id}: {e}");
        }
    }
    let fact_line = if placed {
        "It was raised against work you are not a member of, so what the agent wrote is not repeated here."
    } else {
        "It was raised against a ticket the agent was refused, so it is not an org-wide report and what the agent wrote is not repeated here."
    };
    let fact_tail = if ticket_note == "Helpdesk ticket filed" {
        "The technical details are on the Helpdesk board."
    } else {
        "No Helpdesk ticket could be filed for it."
    };
    for user_id in &who.fact {
        if let Err(e) = add_notification(
            &notify,
            user_id,
            &NotificationInput {
                kind: "agent-problem",
                title: &format!("{label} reported a problem it is blocked on"),
                body: Some(&format!("{fact_line}\n\n{fact_tail}")),
                href: None,
            },
        )
        .await
        {
            tracing::error!(
                "[agent-problem] could not notify {user_id} that a problem exists: {e}"
            );
        }
    }

    // both possible ticket notes contain "filed", so the contains-check is
    // always true and the relay always says a ticket was filed — quirk kept
    // as-is.
    let filed_word = if ticket_note.to_lowercase().contains("filed") {
        "helpdesk ticket was filed"
    } else {
        "report was sent"
    };
    Json(json!({
        "ok": true,
        "ticket": if href != "/observability/alerts" { json!(href) } else { Value::Null },
        // The exact reassurance the agent should relay, so the wording stays
        // consistent and plain.
        "relay": format!("The workspace admin has been notified and a {filed_word} — no action needed on your side."),
    }))
    .into_response()
}
