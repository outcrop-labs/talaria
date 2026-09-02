// /api/tasks/{id}. One ticket.
// GET → full detail (task, comments, activity, watchers, reviews, judge,
// links, usage; + workflows for an agent caller). PUT → update; the
// human-in-the-loop guardrails live in update_task, not here — agents may
// triage but cannot self-assign or move to done. DELETE → owner/editor.

use crate::agent_auth::{AgentSubject, agent_caller};
use crate::boards::{board_allows_agent, board_role, can_edit, invalid_assignee, list_members};
use crate::body::{
    NumKind, as_object, nullish_datetime_member, nullish_enum_member, nullish_member,
    optional_boolean_member, optional_enum_member, optional_max_string_member,
    optional_number_member, optional_string_array_member, optional_string_member,
    optional_uuid_array_member, optional_uuid_member, parse,
};
use crate::error::{house_error, thrown_internal_error};
use crate::mentions::{Mentionee, notify_mentions};
use crate::notify::NotifyDeps;
use crate::refs::{MessageRef, RefChip, RefUser, resolve_refs};
use crate::session::{SessionUser, require_user};
use crate::state::AppState;
use crate::task_const::{EFFORTS, PRIORITIES, TICKET_COLORS};
use crate::tasks::{
    TaskActor, TaskDeps, TaskError, TaskPatch, delete_task, get_task, get_task_full, update_task,
};
use crate::uploads::resolve_attachments;
use crate::workflows::MatchTarget;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(gate) = crate::params::uuid_gate("tasks", "GET task", &id) {
        return gate;
    }
    // 404 comes BEFORE any auth: a ticket's existence is not revealed by who
    // asks about it — unknown id and no session answer the same 404.
    let full = match get_task_full(&state.pg, &id).await {
        Ok(Some(f)) => f,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[tasks] full read failed: {e}");
            return thrown_internal_error();
        }
    };
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return gate,
    };
    if let Some(caller) = caller {
        // The CALLER, not its model — board policy's elevated bypass is only
        // for an identity that was proven, never merely asserted.
        let allowed = match board_allows_agent(
            &state.pg,
            &full.task.board_id,
            &AgentSubject::Caller(caller),
        )
        .await
        {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("[tasks] agent policy read on GET task failed: {e}");
                return thrown_internal_error();
            }
        };
        if !allowed {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
        let target = MatchTarget {
            title: &full.task.title,
            description: full.task.description.as_deref(),
            tags: &full.task.tags,
            board_id: &full.task.board_id,
        };
        let workflows = match crate::workflows::workflows_for_task(&state.pg, &target).await {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("[tasks] workflow read on GET task failed: {e}");
                return thrown_internal_error();
            }
        };
        // The detail body plus a one-off `workflows` payload — the list an
        // agent caller dispatches against.
        let mut body = match serde_json::to_value(&full) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[tasks] full detail serialize failed: {e}");
                return thrown_internal_error();
            }
        };
        if let Some(obj) = body.as_object_mut() {
            obj.insert(
                "workflows".into(),
                serde_json::to_value(&workflows).unwrap_or(Value::Array(vec![])),
            );
        }
        return Json(body).into_response();
    }
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match board_role(&state.pg, &user.id, &full.task.board_id).await {
        Ok(Some(_)) => Json(full).into_response(),
        Ok(None) => house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[tasks] role read on GET task failed: {e}");
            thrown_internal_error()
        }
    }
}

/// The PUT's `refs` member — an optional array (max 3) of {type, id}
/// objects. Nested, so it reads by hand: the messages follow element-then-
/// key order, and the first error is the one answered.
fn refs_member(obj: &serde_json::Map<String, Value>) -> Result<Option<Vec<MessageRef>>, String> {
    match obj.get("refs") {
        None => Ok(None),
        Some(Value::Null) => Err("Invalid input: expected array, received null".into()),
        Some(Value::Array(items)) => {
            if items.len() > 3 {
                return Err(crate::body::array_too_big_msg(3));
            }
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let Some(entry) = item.as_object() else {
                    return Err(format!(
                        "Invalid input: expected object, received {}",
                        crate::body::zod_type_name(item)
                    ));
                };
                let ref_type = crate::body::enum_member(entry, "type", &["kb-doc", "artifact"])?;
                let id = crate::body::uuid_member(entry, "id")?;
                out.push(MessageRef { ref_type, id });
            }
            Ok(Some(out))
        }
        Some(v) => Err(format!(
            "Invalid input: expected array, received {}",
            crate::body::zod_type_name(v)
        )),
    }
}

/// One PUT caller: an agent that passed board policy, or a human that can
/// edit. Carries the session user for the ref ACL (agents attach uploads but
/// not knowledge/artifact refs — no session to check against).
enum PatchActor {
    Agent { model: String },
    Human { actor: TaskActor, user: SessionUser },
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    if let Some(gate) = crate::params::uuid_gate("tasks", "PUT task", &id) {
        return gate;
    }
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[tasks] read on PUT task failed: {e}");
            return thrown_internal_error();
        }
    };
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return gate,
    };
    let who = if let Some(caller) = caller {
        // Identity comes from the credential, so board policy is
        // unconditional — there is no unnamed caller to wave through. Pass
        // the CALLER so the elevated-assistant bypass sees `legacy`.
        let allowed = match board_allows_agent(
            &state.pg,
            &task.board_id,
            &AgentSubject::Caller(caller.clone()),
        )
        .await
        {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("[tasks] agent policy read on PUT task failed: {e}");
                return thrown_internal_error();
            }
        };
        if !allowed {
            return house_error(
                StatusCode::FORBIDDEN,
                &format!("agent \"{}\" is not allowed on this board", caller.model),
            );
        }
        PatchActor::Agent {
            model: caller.model,
        }
    } else {
        let user = match require_user(&state, &headers).await {
            Ok(u) => u,
            Err(gate) => return gate,
        };
        let role = match board_role(&state.pg, &user.id, &task.board_id).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("[tasks] role read on PUT task failed: {e}");
                return thrown_internal_error();
            }
        };
        if !can_edit(role.as_deref()) {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
        let actor = TaskActor::human(
            user.email
                .clone()
                .or_else(|| user.name.clone())
                .unwrap_or_else(|| "user".into()),
        );
        PatchActor::Human { actor, user }
    };
    let (actor, session_user) = match who {
        PatchActor::Agent { model } => (TaskActor::agent(model), None),
        PatchActor::Human { actor, user } => (actor, Some(user)),
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Patch, in schema declaration order — the FIRST failing field is the
    // one answered.
    // `status`'s min(1) is not cosmetic: `""` was once a legal patch value,
    // and it is falsy, so every presence-turned-truthiness guard in
    // update_task skipped on a write that then stored an empty column — the
    // route schema and the library both refuse it now.
    let title = match optional_string_member(obj, "title", 300) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let description = match nullish_member(obj, "description", |o, k| {
        optional_max_string_member(o, k, 20_000)
    }) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let status = match optional_string_member(obj, "status", 40) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let priority = match optional_enum_member(obj, "priority", PRIORITIES) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let effort = match nullish_member(obj, "effort", |o, k| nullish_enum_member(o, k, EFFORTS)) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let assignees = match optional_string_array_member(obj, "assignees", 0, 200, 20) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let due_date = match nullish_member(obj, "dueDate", nullish_datetime_member) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let start_date = match nullish_member(obj, "startDate", nullish_datetime_member) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let color = match nullish_member(obj, "color", |o, k| {
        nullish_enum_member(o, k, TICKET_COLORS)
    }) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // tags elements carry min(1) — `[""]` once minted a blank label on the
    // board. `[]` stays legal: it clears the labels.
    let tags = match optional_string_array_member(obj, "tags", 1, 40, 20) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let outcome = match nullish_member(obj, "outcome", |o, k| {
        optional_max_string_member(o, k, 50_000)
    }) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let resolution = match nullish_member(obj, "resolution", |o, k| {
        optional_max_string_member(o, k, 50_000)
    }) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let error_message = match nullish_member(obj, "errorMessage", |o, k| {
        optional_max_string_member(o, k, 50_000)
    }) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let archived = match optional_boolean_member(obj, "archived") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let estimated_hours = match nullish_member(obj, "estimatedHours", |o, k| {
        crate::body::nullable_number_member(o, k, NumKind::Float, 0.0, 999.0)
    }) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let parent_id = match nullish_member(obj, "parentId", optional_uuid_member) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let add_time_spent_seconds = match optional_number_member(
        obj,
        "addTimeSpentSeconds",
        NumKind::Float,
        0.0,
        86_400.0 * 30.0,
    ) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Full replacement list, same contract as chat messages: upload ids +
    // knowledge/artifact refs. Omit both to leave attachments unchanged.
    let attachment_ids = match optional_uuid_array_member(obj, "attachmentIds", 20) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let refs = match refs_member(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Human-in-the-loop guardrails (assignment, sign-off, archival) belong
    // to the ACTOR, not this route: update_task enforces them for every
    // caller, so nothing agent-specific happens here.
    match invalid_assignee(
        &state.pg,
        &task.board_id,
        assignees.as_deref().unwrap_or(&[]),
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(bad)) => return house_error(StatusCode::BAD_REQUEST, &bad),
        Err(e) => {
            tracing::error!("[tasks] assignee check on PUT task failed: {e}");
            return thrown_internal_error();
        }
    }
    let attachments = if attachment_ids.is_some() || refs.is_some() {
        // Resolve to canonical chips server-side (never trust client
        // metadata). Refs are ACL-checked against the attacher, so agent
        // callers (no session) attach uploads but not knowledge/artifact
        // refs.
        let uploads =
            match resolve_attachments(&state.pg, attachment_ids.as_deref().unwrap_or(&[])).await {
                Ok(u) => u,
                Err(e) => {
                    tracing::error!("[tasks] attachment resolve failed: {e}");
                    return thrown_internal_error();
                }
            };
        let chips: Vec<RefChip> = if let Some(user) = session_user.as_ref() {
            let ref_user = RefUser {
                id: &user.id,
                email: user.email.as_deref(),
                name: user.name.as_deref(),
            };
            match resolve_refs(&state.pg, &ref_user, refs.as_deref().unwrap_or(&[])).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("[tasks] ref resolve failed: {e}");
                    return thrown_internal_error();
                }
            }
        } else {
            Vec::new()
        };
        let mut list: Vec<Value> = uploads
            .iter()
            .filter_map(|u| serde_json::to_value(u).ok())
            .collect();
        list.extend(chips.iter().filter_map(|c| serde_json::to_value(c).ok()));
        Some(Value::Array(list))
    } else {
        None
    };
    // The mention check (below, after the write) reads the description the
    // PATCH carried, so it survives the patch's move into update_task.
    let description_input = description.clone();
    let patch = TaskPatch {
        title,
        description,
        status,
        priority,
        effort,
        assignees,
        due_date,
        start_date,
        color,
        tags,
        outcome,
        resolution,
        error_message,
        archived,
        estimated_hours,
        parent_id,
        attachments,
        add_time_spent_seconds,
        status_note: None,
    };
    let deps = TaskDeps::from_route(state.pg.clone(), state.redis().await.ok());
    let updated = match update_task(&deps, &id, patch, &actor).await {
        Ok(t) => t,
        Err(TaskError::ApprovalRequired(msg)) => return house_error(StatusCode::FORBIDDEN, &msg),
        Err(TaskError::Refusal(msg)) => return house_error(StatusCode::BAD_REQUEST, &msg),
        Err(TaskError::Db(e)) => {
            tracing::error!("[tasks] update failed: {e}");
            return thrown_internal_error();
        }
    };
    // No inline index or judge trigger on this path: a text edit reaches the
    // activity brain only through the opportunistic RAG sweep (keyed on
    // updated_at, at most 15 min behind), and entering a review-category
    // column fires no judge run — the reviewer gets no advisory verdict.
    if let Some(updated) = updated.as_ref()
        && description_input.is_some()
        && description_input != Some(task.description.clone())
    {
        // A description that GAINS an @mention notifies board members —
        // same contract as comments. Only on actual change, never on
        // other patches.
        let (sender_id, sender_label) = match session_user.as_ref() {
            Some(user) => (
                user.id.clone(),
                user.name.clone().unwrap_or_else(|| actor.id.clone()),
            ),
            None => (String::new(), crate::fleet::describe_agent(&actor.id).label),
        };
        let pg = state.pg.clone();
        let notify = NotifyDeps::publishing(pg.clone(), state.redis().await.ok());
        let board_id = task.board_id.clone();
        let task_id = task.id.clone();
        let content = description_input.clone().flatten().unwrap_or_default();
        let where_ = updated
            .ticket_ref
            .clone()
            .unwrap_or_else(|| "a ticket".into());
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
    Json(json!({ "task": updated })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("tasks", "DELETE task", &id) {
        return gate;
    }
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[tasks] read on DELETE task failed: {e}");
            return thrown_internal_error();
        }
    };
    let role = match board_role(&state.pg, &user.id, &task.board_id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[tasks] role read on DELETE task failed: {e}");
            return thrown_internal_error();
        }
    };
    if !can_edit(role.as_deref()) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    // Comments ride the task's foreign keys, so their ids are read before the
    // row dies — the delete takes the ticket's and every comment's activity
    // point out of the brain too, so no dead text answers searches while a
    // reindex is nowhere in sight.
    let comment_ids: Vec<String> =
        sqlx::query_scalar("select id::text from task_comments where task_id = $1::uuid")
            .bind(&id)
            .fetch_all(&state.pg)
            .await
            .unwrap_or_default();
    let deps = TaskDeps::from_route(state.pg.clone(), state.redis().await.ok());
    match delete_task(&deps, &id).await {
        Ok(()) => {
            // Fire-and-forget, like the board and channel deletes.
            let pg = state.pg.clone();
            let ticket_id = id.clone();
            tokio::spawn(async move {
                let qd = crate::retrieval::qdrant::real_deps();
                let ed = crate::retrieval::embed::real_deps();
                let _ = crate::retrieval::sources::unindex_activity(
                    &pg, &qd, &ed, "ticket", &ticket_id,
                )
                .await;
                for cid in comment_ids {
                    let _ =
                        crate::retrieval::sources::unindex_activity(&pg, &qd, &ed, "comment", &cid)
                            .await;
                }
            });
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => {
            tracing::error!("[tasks] delete failed: {e}");
            thrown_internal_error()
        }
    }
}
