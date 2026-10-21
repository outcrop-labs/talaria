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
//
// THE BOARD IS RESOLVED BY ITS STORED ID, NEVER BY NAME, AND NEVER REWRITTEN.
// The old find-by-name path matched ANY live board called "Helpdesk" — a
// support lead's private team board included — and on every call made it
// org-wide, moved it to the Talaria team, joined every user as editor and
// reset its agent policy. An agent key must not be able to re-share a board
// a person owns, and even the system's own board must keep the membership
// removals and agent restrictions an admin made. So: the id lives in
// `app_settings` (`helpdesk_board_id`), the board is created exactly once —
// org-wide, on the Talaria team, open to every agent — and a stored board
// that has since been archived or deleted is replaced by a fresh one rather
// than un-archived.
//
// BRAKED AND DEDUPED. Each call files a ticket and notifies every admin, so
// an agent gets a per-hour budget, and the same (agent, summary) within a
// day is acknowledged without filing or notifying again — the posture
// `report_gap` has always had.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_auth::{AgentSubject, require_agent};
use talaria_api_facades::fleet::describe_agent;
use talaria_api_facades::runs::define::Authority;
use talaria_approvals::audience_for;
use talaria_boards::{board_info, create_board, join_everyone_to_board, set_board_agent_config};
use talaria_body::{optional_max_string_member, parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_gaps::{agent_text_authority, remember_ticket_refusal};
use talaria_notify::{NotificationInput, NotifyDeps, add_notification};
use talaria_ratelimit::{brake, rate_limit};
use talaria_settings::{get_setting, set_setting};
use talaria_state::AppState;
use talaria_tasks::{
    AgentIntent, AgentWriteTarget, NewTask, ResolvedTaskId, TaskDeps, agent_ticket_refusal,
    create_task, get_task, resolve_task_id,
};
use talaria_teams::create_team;

const HELPDESK: &str = "Helpdesk";
const TALARIA: &str = "Talaria";
/// Where the Helpdesk board's id is kept — written once, on creation.
const HELPDESK_BOARD_KEY: &str = "helpdesk_board_id";
/// Problem reports an agent may file per hour.
const PROBLEMS_PER_HOUR: i64 = 10;
/// The window one (agent, summary) is acknowledged without a second ticket.
const DEDUPE_WINDOW_SECONDS: i64 = 24 * 60 * 60;

/// The dedupe atom for a summary: lowercase, alphanumerics only, so
/// punctuation and spacing differences land on the same key.
fn summary_key(summary: &str) -> String {
    summary
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .take(200)
        .collect()
}

/// The Helpdesk board's id: the stored one when it is still a live board,
/// else a board created once and remembered. An EXISTING board is never
/// written to here — not its sharing, not its team, not its members, not
/// its agent policy.
async fn helpdesk_board(pg: &sqlx::PgPool) -> Option<String> {
    let stored = get_setting(pg, HELPDESK_BOARD_KEY, Value::Null).await;
    if let Some(id) = stored.as_str().filter(|s| !s.trim().is_empty()) {
        match board_info(pg, id).await {
            Ok(info) if info.exists && info.archived_at.is_none() => return Some(id.to_string()),
            // Archived or deleted: a person retired it, so a new one is
            // created below rather than the old one resurrected.
            Ok(_) => {}
            Err(e) => {
                tracing::error!("[agent.problem] helpdesk board read failed: {e}");
                return None;
            }
        }
    }
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
    let board = create_board(pg, &admin_id, HELPDESK, Some(&team_id))
        .await
        .ok()?;
    // Org-wide, everyone an editor, every agent allowed — set ONCE, at
    // creation. From here on an admin's changes to the board stand.
    sqlx::query("update boards set org_wide = true where id = $1::uuid")
        .bind(&board.id)
        .execute(pg)
        .await
        .ok()?;
    join_everyone_to_board(pg, &board.id).await.ok()?;
    set_board_agent_config(pg, &board.id, true, &[])
        .await
        .ok()?;
    if let Err(e) = set_setting(pg, HELPDESK_BOARD_KEY, &json!(board.id)).await {
        tracing::error!("[agent.problem] helpdesk board id could not be stored: {e}");
    }
    Some(board.id)
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return Ok(resp),
    };
    let agent = caller.model.clone();
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let summary = match string_member(obj, "summary", 5, 300) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let details = match optional_max_string_member(obj, "details", 20_000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // what the agent was trying to do
    let context = match optional_max_string_member(obj, "context", 500) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let label = describe_agent(&agent).label;
    // The per-agent budget. Redis-backed and failing open, like every
    // limiter in this house.
    let mut redis = state.redis().await.ok();
    if let Some(r) = redis.as_mut()
        && let Some(gate) = brake(
            r,
            &format!("agent:problem:{agent}"),
            PROBLEMS_PER_HOUR,
            3600,
            "too many problem reports from this agent — the ones already filed are with the admins; try again later",
        )
        .await
    {
        return Ok(gate);
    }
    // the ticket the agent was working when it broke
    let named = match optional_max_string_member(obj, "taskId", 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let (task_id, task) = match named.as_deref() {
        None => (None, None),
        Some(raw) => match resolve_task_id(&state.pg, raw).await {
            Ok(ResolvedTaskId::One(id)) => match get_task(&state.pg, &id).await {
                Ok(t) => (Some(id), t),
                Err(e) => return Ok(internal("[agent.problem] ticket read failed", e)),
            },
            Ok(_) => (Some(raw.to_string()), None),
            Err(e) => return Ok(internal("[agent.problem] ticket lookup failed", e)),
        },
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
            return Ok(refuse());
        };
        // The CALLER, not its model: board policy's elevated bypass is only
        // for an identity that was proven, never merely asserted.
        let allowed = match talaria_boards::board_allows_agent(
            &state.pg,
            &task.board_id,
            &AgentSubject::Caller(caller.clone()),
        )
        .await
        {
            Ok(v) => v,
            Err(e) => return Ok(internal("[agent.problem] board policy read failed", e)),
        };
        if !allowed {
            remember_ticket_refusal(&state.pg, &agent, None).await;
            return Ok(refuse());
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
                return Ok(resp);
            }
            Err(e) => return Ok(internal("[agent.problem] ticket refusal read failed", e)),
        }
    }

    // A repeat of the same (agent, summary) within the window is acknowledged
    // and nothing more: no second ticket, no second round of notifications.
    // The key is consumed only on an AUTHORISED report, so a refusal above
    // never spends the honest retry's turn.
    if let Some(r) = redis.as_mut() {
        let dup = rate_limit(
            r,
            &format!("agent:problem:dup:{agent}:{}", summary_key(&summary)),
            1,
            DEDUPE_WINDOW_SECONDS,
        )
        .await;
        if !dup.ok {
            return Ok(Json(json!({
                "ok": true,
                "ticket": Value::Null,
                "duplicate": true,
                "relay": "This problem was already reported to the workspace admin in the last day — no action needed on your side.",
            }))
            .into_response());
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
                return Ok(internal(
                    "[agent.problem] helpdesk filing failed",
                    e.message(),
                ));
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
                // THE ALERTS PAGE, never the ticket, even when one was filed:
                // the fact audience is precisely the people NOT on the board
                // (board visibility is membership, not role), so a board href
                // would be dropped by the brief's accessibility check for
                // every reader this row is for. And None made the row
                // invisible to the brief outright — it lists only
                // notifications with somewhere to go, and a problem an admin
                // cannot click through to is a headline, not a notice.
                href: Some("/observability/alerts"),
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
    Ok(
    Json(json!({
        "ok": true,
        "ticket": if href != "/observability/alerts" { json!(href) } else { Value::Null },
        // The exact reassurance the agent should relay, so the wording stays
        // consistent and plain.
        "relay": format!("The workspace admin has been notified and a {filed_word} — no action needed on your side."),
    }))
    .into_response())
}
