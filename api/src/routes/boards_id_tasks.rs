// /api/boards/{id}/tasks — port of ui/src/routes/api/boards.$id.tasks.ts.
// GET → the board's tasks (any member, or a board-allowed agent; only humans
// may ask for the archived tail). POST → create a card (owner/editor, or a
// board-allowed agent → inbox). The create's guardrails ride in the library
// (create_task); this file holds the actor resolution and the body.

use crate::agent_auth::{AgentSubject, agent_caller};
use crate::boards::{board_allows_agent, board_role, can_edit, invalid_assignee, list_members};
use crate::body::{
    as_object, nullish_datetime_member, nullish_enum_member, optional_enum_member,
    optional_max_string_member, optional_string_array_member, optional_uuid_member, parse,
    string_member,
};
use crate::error::house_error;
use crate::mentions::{Mentionee, notify_mentions};
use crate::notify::NotifyDeps;
use crate::session::require_user;
use crate::state::AppState;
use crate::task_const::{EFFORTS, PRIORITIES, TICKET_COLORS};
use crate::tasks::{NewTask, create_task, list_board_tasks};
use crate::templates::{ResolveContext, resolve_template};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Resolves who's calling: a board-allowed agent (by key + name), an editing
/// user, or nobody. Agents must pass the board's agent policy.
enum TaskActor {
    Agent {
        model: String,
    },
    Human {
        actor: String,
        user: crate::session::SessionUser,
    },
}

async fn task_actor(
    state: &AppState,
    headers: &HeaderMap,
    board_id: &str,
    require_edit: bool,
    action: &str,
) -> Result<TaskActor, Response> {
    let caller = agent_caller(&state.pg, headers).await?;
    if let Some(caller) = caller {
        // Pass the CALLER, not its model: board policy's elevated-assistant
        // bypass is org-wide reach, and only a caller that PROVED its
        // identity gets it.
        let allowed =
            board_allows_agent(&state.pg, board_id, &AgentSubject::Caller(caller.clone()))
                .await
                .map_err(|e| {
                    tracing::error!("[tasks] agent policy read on {action} failed: {e}");
                    house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
                })?;
        if !allowed {
            let model = caller.model.clone();
            return Err(house_error(
                StatusCode::FORBIDDEN,
                &format!("agent \"{model}\" is not allowed on this board"),
            ));
        }
        return Ok(TaskActor::Agent {
            model: caller.model,
        });
    }
    let user = require_user(state, headers).await?;
    let role = board_role(&state.pg, &user.id, board_id)
        .await
        .map_err(|e| {
            tracing::error!("[tasks] role read on {action} failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        })?;
    let ok = if require_edit {
        can_edit(role.as_deref())
    } else {
        role.is_some()
    };
    if !ok {
        return Err(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let actor = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "user".into());
    Ok(TaskActor::Human { actor, user })
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    uri: Uri,
) -> Response {
    let who = match task_actor(&state, &headers, &id, false, "GET board tasks").await {
        Ok(w) => w,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "GET tasks", &id) {
        return gate;
    }
    // The archived tail is asked for by name, and only by humans — an agent
    // has no business trawling retired work.
    let include_archived = match &who {
        TaskActor::Agent { .. } => false,
        TaskActor::Human { .. } => {
            uri.query()
                .and_then(|q| {
                    url::form_urlencoded::parse(q.as_bytes())
                        .find(|(k, _)| k == "archived")
                        .map(|(_, v)| v.into_owned())
                })
                .as_deref()
                == Some("1")
        }
    };
    match list_board_tasks(&state.pg, &id, include_archived).await {
        Ok(tasks) => Json(json!({ "tasks": tasks })).into_response(),
        Err(e) => {
            tracing::error!("[tasks] board list failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let who = match task_actor(&state, &headers, &id, true, "POST board tasks").await {
        Ok(w) => w,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "POST tasks", &id) {
        return gate;
    }
    let (is_agent, actor, session_user) = match who {
        TaskActor::Agent { model } => (true, model, None),
        TaskActor::Human { actor, user } => (false, actor, Some(user)),
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let title = match string_member(obj, "title", 1, 300) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let description = match optional_max_string_member(obj, "description", 20_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let priority = match optional_enum_member(obj, "priority", PRIORITIES) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let effort = match nullish_enum_member(obj, "effort", EFFORTS) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let assignees = match optional_string_array_member(obj, "assignees", 0, 200, 20) {
        Ok(v) => v.unwrap_or_default(),
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let due_date = match nullish_datetime_member(obj, "dueDate") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let start_date = match nullish_datetime_member(obj, "startDate") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let color = match nullish_enum_member(obj, "color", TICKET_COLORS) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let estimated_hours = match crate::body::nullable_number_member(
        obj,
        "estimatedHours",
        crate::body::NumKind::Float,
        0.0,
        999.0,
    ) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let parent_id = match optional_uuid_member(obj, "parentId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let tags = match optional_string_array_member(obj, "tags", 0, 40, 20) {
        Ok(v) => v.unwrap_or_default(),
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Guardrail: agents create into inbox only — assignment stays a human
    // call.
    if is_agent && !assignees.is_empty() {
        return house_error(StatusCode::FORBIDDEN, "agents cannot assign tickets");
    }
    // The same human-planning fields update_task strips from an agent PATCH
    // (estimate, sub-task structure) are not an agent's to set at CREATION
    // either — otherwise a hand-rolled POST walks around the update gate and
    // an agent estimates its own work or re-parents the plan. Dropped rather
    // than refused, so the ticket still lands: same end state as creating it
    // and then being unable to patch these in.
    let (estimated_hours, parent_id) = if is_agent {
        (None, None)
    } else {
        (estimated_hours, parent_id)
    };
    // Mixed assignees: `user:<uuid>` must be a board member; bare strings
    // are agents and must pass the board's agent policy.
    match invalid_assignee(&state.pg, &id, &assignees).await {
        Ok(None) => {}
        Ok(Some(bad)) => return house_error(StatusCode::BAD_REQUEST, &bad),
        Err(e) => {
            tracing::error!("[tasks] assignee check failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    }
    // The mention test runs against the RAW body description (below), before
    // template seeding overwrites it.
    let raw_description = description.clone();
    // Templatize bare tickets: an empty description is seeded from the
    // resolved ticket template (creating agent's binding → board default),
    // so every creation surface — quick-add, agent tools — gets the format.
    let mut description = description;
    if !description.as_deref().is_some_and(|d| !d.trim().is_empty()) {
        let ctx = ResolveContext {
            explicit_id: None,
            agent_model: if is_agent { Some(actor.as_str()) } else { None },
            board_id: Some(&id),
        };
        match resolve_template(&state.pg, "ticket", &ctx).await {
            Ok(Some(t)) if !t.body.trim().is_empty() => description = Some(t.body),
            Ok(_) => {}
            Err(e) => {
                tracing::error!("[tasks] template resolve on create failed: {e}");
                return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
            }
        }
    }
    let input = NewTask {
        board_id: &id,
        title: &title,
        description: description.as_deref(),
        priority: priority.as_deref(),
        effort: effort.as_deref(),
        assignees: &assignees,
        due_date: due_date.as_deref(),
        start_date: start_date.as_deref(),
        color: color.as_deref(),
        estimated_hours,
        parent_id: parent_id.as_deref(),
        tags: &tags,
        created_by: &actor,
    };
    let deps = crate::tasks::TaskDeps::coexistence(state.pg.clone(), state.redis().await.ok());
    let task = match create_task(&deps, &input).await {
        Ok(t) => t,
        Err(crate::tasks::TaskError::Db(e)) => {
            tracing::error!("[tasks] create failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
        Err(e) => return e.into_response(),
    };
    // NOT YET CROSSED: TS indexes the new ticket into the ambient activity
    // brain (indexTicket, detached) — that leg is the retrieval plane and
    // crosses with batch 5. Until then a ticket created through Rust is
    // absent from the board-scoped activity index until its next edit.
    //
    // A description born with an @mention notifies board members — same
    // contract as editing one in (tasks/{id} PUT). Template seeds carry no
    // mentions, so only author-written descriptions can fire this: the test
    // is the RAW body description, not the seeded one.
    if raw_description.as_deref().is_some_and(|d| d.contains('@')) {
        // sender: the agent by its label, the human by session name falling
        // back to the actor string.
        let (sender_id, sender_label) = if is_agent {
            (String::new(), crate::fleet::describe_agent(&actor).label)
        } else {
            let u = session_user.as_ref();
            (
                u.map(|u| u.id.clone()).unwrap_or_default(),
                u.and_then(|u| u.name.clone())
                    .unwrap_or_else(|| actor.clone()),
            )
        };
        let pg = state.pg.clone();
        let notify = NotifyDeps::publishing(pg.clone(), state.redis().await.ok());
        let board_id = id.clone();
        let content = raw_description.clone().unwrap_or_default();
        let where_ = task.ticket_ref.clone().unwrap_or_else(|| "a ticket".into());
        let task_id = task.id.clone();
        tokio::spawn(async move {
            let Ok(members) = list_members(&pg, &board_id).await else {
                return;
            };
            let members: Vec<Mentionee> = members
                .into_iter()
                .map(|m| Mentionee {
                    user_id: m.user_id,
                    name: m.name,
                    email: m.email,
                })
                .collect();
            let href = format!("/boards/{board_id}/{task_id}");
            let _ = notify_mentions(
                &notify,
                &members,
                &sender_id,
                &sender_label,
                &content,
                &where_,
                &href,
            )
            .await;
        });
    }
    Json(json!({ "task": task })).into_response()
}
