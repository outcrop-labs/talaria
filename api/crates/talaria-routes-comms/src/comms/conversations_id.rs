// /api/conversations/{id}.
// GET → the conversation + its messages (ownership-checked). PATCH { title }
// → rename (owner, or a plan collaborator). A renamed title no longer matches
// the mechanical first-message truncation, so the Titler and its sweep leave
// it alone from then on. PATCH { archived: false } restores a plan the owner
// archived — the one way back, and owner-only, because archive hides the row
// from every member's list. DELETE archives (owner); ?hard=1 hard-deletes
// (owner). Decay never sets this flag on a plan (it selects kind = 'chat'),
// so archived on a plan means a person put it away.
//
// Research runs are not archived here. A run is ephemeral: delete is its stop.
//
// PATCH { pinned, group, position } is a different kind of write from the
// rest: it says nothing about the conversation, only about where the CALLER
// keeps it — their own pin, their own heading, their own slot in the list. So
// any role the GET would show may write it, collaborator included, and it
// lands in that person's `conversation_layout` row instead of on the thread
// everybody shares. The validation lives in talaria-conversation-layout
// because the agent's organise door has to refuse exactly what this one does.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use talaria_body::{optional_boolean_member, trimmed_string_member};
use talaria_conversations::{get_conversation, set_pinned_files};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::{
    NotifyDeps, conversation_audience_ids, fan_conversation_event, fan_known_conversation,
};
use talaria_plan_drafts::drop_draft;
use talaria_session::require_user;
use talaria_state::AppState;

#[derive(serde::Serialize)]
struct DetailEnvelope {
    conversation: talaria_conversations::ConversationDetail,
    messages: Vec<talaria_conversations::MessageRow>,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(match get_conversation(&state.pg, &user.id, &id).await {
        Ok(Some((conversation, messages))) => (
            StatusCode::OK,
            Json(DetailEnvelope {
                conversation,
                messages,
            }),
        )
            .into_response(),
        Ok(None) => house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => internal("[conversations] detail read failed", e),
    })
}

/// `pinnedFiles` → a validated jsonb array. Each entry must name a kind this
/// client knows how to render and an id of a sane shape; anything else is a
/// 400 rather than a pane that renders an empty iframe and blames Google.
///
/// The cap is deliberate and small: this is the pane's memory, not a file
/// manager, and an unbounded array is a jsonb column somebody can grow for
/// ever.
fn pinned_files_member(obj: &serde_json::Map<String, Value>) -> Result<Value, String> {
    const MAX_FILES: usize = 12;
    let Some(items) = obj.get("pinnedFiles").and_then(Value::as_array) else {
        return Err("Invalid input: expected pinnedFiles to be an array".into());
    };
    if items.len() > MAX_FILES {
        return Err(format!("Too many pinned files: {MAX_FILES} at most"));
    }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(entry) = item.as_object() else {
            return Err("Invalid input: expected each pinned file to be an object".into());
        };
        let kind = entry
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind != "google" && kind != "artifact" {
            return Err("Invalid option: expected kind to be \"google\" or \"artifact\"".into());
        }
        let id = entry.get("id").and_then(Value::as_str).unwrap_or_default();
        // Google file ids and Talaria uuids are both in this alphabet; the
        // length cap is what keeps a pasted URL out of the column.
        if id.is_empty()
            || id.len() > 200
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("Invalid input: expected id to be a file id".into());
        }
        let mut kept = serde_json::Map::new();
        kept.insert("kind".into(), Value::String(kind.to_string()));
        kept.insert("id".into(), Value::String(id.to_string()));
        for optional in ["title", "mime"] {
            if let Some(v) = entry
                .get(optional)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
            {
                kept.insert(
                    optional.into(),
                    Value::String(v.chars().take(400).collect()),
                );
            }
        }
        out.push(Value::Object(kept));
    }
    Ok(Value::Array(out))
}

#[derive(serde::Serialize)]
struct OkTrue {
    ok: bool,
}

/// The access gate IS the read: a mutation runs only on a conversation the
/// GET would show this caller. None is 404 — a stranger probing ids learns
/// nothing. The role comes back with that read, so owner-only arms don't
/// take a second query.
async fn visible_role(
    state: &AppState,
    user_id: &str,
    id: &str,
    where_: &str,
) -> Result<String, Response> {
    match get_conversation(&state.pg, user_id, id).await {
        Ok(Some((detail, _))) => Ok(detail.role),
        Ok(None) => Err(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => Err(internal(where_, e)),
    }
}

async fn fan(state: &AppState, id: &str) {
    fan_conversation_event(
        NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok()),
        id.to_string(),
    );
}

/// ?hard=1 — exact match — hard-deletes. Any other value, including absent,
/// archives. Same spelling as DELETE /api/channels/{id}.
fn hard_delete(uri: &Uri) -> bool {
    uri.query()
        .and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find(|(k, _)| k == "hard")
                .map(|(_, v)| v.into_owned())
        })
        .as_deref()
        == Some("1")
}

// doc: PATCH takes any of five writes. `title` (1..120, trimmed) renames the
// doc: thread. `archived: false` restores one the owner archived — owner
// doc: only, and archiving itself is DELETE. `pinnedFiles` replaces the
// doc: document pane's list (at most 12 entries, each
// doc: { kind: "google" | "artifact", id, title?, mime? }). And three fields
// doc: that say where the CALLER keeps this thread rather than anything about
// doc: the thread: `pinned` (bool), `group` (trimmed string, 40 characters at
// doc: most) and `position` (whole number, 0 to 100000). On group and
// doc: position, null is the explicit clear and an absent field is left
// doc: alone; those three land in the caller's own layout row, so any role
// doc: the GET would show may write them, collaborator included, while title
// doc: and archived stay owner (or plan-collaborator) writes. A body naming
// doc: none of the five is a 400.
pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let role = visible_role(&state, &user.id, &id, "[conversations] gate read failed").await?;
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let title = if obj.contains_key("title") {
        match trimmed_string_member(obj, "title", 1, 120) {
            Ok(t) => Some(t),
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        }
    } else {
        None
    };
    let archived = match optional_boolean_member(obj, "archived") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The document pane's memory. Validated for SHAPE here rather than trusted:
    // it is rendered as an embed target, so an entry with a junk kind or a
    // missing id would become a broken pane rather than a refused write.
    let pinned = if obj.contains_key("pinnedFiles") {
        match pinned_files_member(obj) {
            Ok(v) => Some(v),
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        }
    } else {
        None
    };
    // WHERE THE CALLER KEEPS THIS THREAD. The three travel as one patch
    // because they share a row and because null means "clear this" rather than
    // "leave it alone" — a distinction the member vocabulary cannot carry. The
    // same reader serves the agent door, so a group this route refuses is a
    // group no agent can file a session under either.
    //
    // NAMED OR ABSENT, nothing in between: a body carrying only a title is not
    // an organise call, and handing it to the reader would turn every rename
    // into the reader's "nothing to change".
    let organising =
        obj.contains_key("pinned") || obj.contains_key("group") || obj.contains_key("position");
    let layout = if organising {
        match talaria_conversation_layout::patch_from(obj) {
            Ok(patch) => Some(patch),
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        }
    } else {
        None
    };
    if title.is_none() && archived.is_none() && pinned.is_none() && layout.is_none() {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "expected title, archived, pinnedFiles, pinned, group or position",
        ));
    }
    // Archive is DELETE. PATCH only restores, so there is one door that
    // hides a plan and one door that brings it back.
    if archived == Some(true) {
        return Ok(house_error(StatusCode::BAD_REQUEST, "archive with DELETE"));
    }
    // Restore hides the row from every member. A collaborator must not be
    // able to put back what the owner put away.
    if archived.is_some() && role != "owner" {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    if let Some(files) = &pinned
        && let Err(e) = set_pinned_files(&state.pg, &id, files).await
    {
        return Ok(internal("[conversations] pinned files write failed", e));
    }
    // Owner or collaborator, with no further gate: this is the caller's own
    // row, and arranging your own list cannot reach anybody else's.
    if let Some(patch) = &layout
        && let Err(e) =
            talaria_conversation_layout::write_layout(&state.pg, &user.id, &id, patch).await
    {
        return Ok(internal("[conversations] layout write failed", e));
    }
    if let Some(title) = &title {
        let updated = sqlx::query("update conversations set title = $1 where id = $2::uuid")
            .bind(title)
            .bind(&id)
            .execute(&state.pg)
            .await;
        if let Err(e) = updated {
            return Ok(internal("[conversations] rename failed", e));
        }
    }
    if archived == Some(false) {
        let updated = sqlx::query(
            "update conversations set archived = false, updated_at = now() where id = $1::uuid",
        )
        .bind(&id)
        .execute(&state.pg)
        .await;
        if let Err(e) = updated {
            return Ok(internal("[conversations] restore failed", e));
        }
        // The schedules the archive paused come back with the thread.
        talaria_agent_crons::set_jobs_paused_for_conversation(&state.pg, &id, false).await;
    }
    fan(&state, &id).await;
    Ok((StatusCode::OK, Json(OkTrue { ok: true })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    uri: Uri,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let role = visible_role(
        &state,
        &user.id,
        &id,
        "[conversations] gate read on delete failed",
    )
    .await?;
    // Both arms are owner-only, the channel pattern: a collaborator renaming
    // a shared plan is fine, hiding it from the owner is not.
    if role != "owner" {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    if hard_delete(&uri) {
        // Resolve the audience before the row goes. The usual fan reads that
        // row, and a spawn racing the delete would ring nobody.
        let audience = match conversation_audience_ids(&state.pg, &id).await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::error!("[conversations] audience read on delete failed: {e}");
                Vec::new()
            }
        };
        // Cancel a live draft before the conversation disappears, so a job
        // mid-spend stops at its next boundary instead of writing into a
        // deleted plan. Older draft rows have no FK — sweep them too.
        if let Err(e) = drop_draft(&state, &id).await {
            return Ok(internal("[conversations] draft cancel failed", e));
        }
        // THE THREAD'S CRONS GO BEFORE THE CASCADE. `conversation_schedules`
        // cascades with the row, but the jobs live in the agent's own
        // jobs.json and know nothing of it: left behind, each fires forever
        // (unmetered) into a door that answers 404. Best-effort — a container
        // that cannot be reached is logged, and the delete still proceeds,
        // because the person asked for the thread to go, not for its agent
        // to be reachable.
        let removed = talaria_agent_crons::remove_jobs_for_conversation(&state.pg, &id).await;
        if !removed.is_empty() {
            tracing::info!(
                "[conversations] {id}: {} scheduled job(s) removed on delete",
                removed.len()
            );
        }
        if let Err(e) = sqlx::query("delete from plan_drafts where conversation_id = $1::uuid")
            .bind(&id)
            .execute(&state.pg)
            .await
        {
            return Ok(internal("[conversations] draft sweep failed", e));
        }
        // A RESEARCH SESSION IS ONE THING WITH TWO ROWS. The FK from the run
        // to its conversation is ON DELETE SET NULL, so a bare delete here
        // would leave the run alive and orphaned — its driver still spending,
        // its report reachable only by an id nobody holds any more. When the
        // thread belongs to a run, the run's own delete takes both (it cancels
        // first, then removes run and conversation in one transaction).
        let run_id = talaria_research::research_run_for_conversation(&state.pg, &id)
            .await
            .ok()
            .flatten();
        if let Some(run_id) = run_id {
            if let Err(e) = talaria_research::delete_research_run(&state, &run_id).await {
                return Ok(internal("[conversations] research delete failed", e));
            }
        } else if let Err(e) = sqlx::query("delete from conversations where id = $1::uuid")
            .bind(&id)
            .execute(&state.pg)
            .await
        {
            return Ok(internal("[conversations] delete failed", e));
        }
        fan_known_conversation(
            NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok()),
            id.clone(),
            audience,
        );
    } else {
        if let Err(e) = sqlx::query(
            "update conversations set archived = true, updated_at = now() where id = $1::uuid",
        )
        .bind(&id)
        .execute(&state.pg)
        .await
        {
            return Ok(internal("[conversations] archive failed", e));
        }
        // An archived thread is one nobody reads: its schedules pause (the
        // rows stay, so a restore resumes the same set) instead of posting
        // turns into a thread that is put away.
        talaria_agent_crons::set_jobs_paused_for_conversation(&state.pg, &id, true).await;
        fan(&state, &id).await;
    }
    Ok((StatusCode::OK, Json(OkTrue { ok: true })).into_response())
}
