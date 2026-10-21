// /api/kb/spaces. KB spaces (any member). GET → all the caller can read
// (agents over MCP see org/public + granted + — for a personal assistant —
// its owner's own; humans see visibility-read + granted). POST → create
// (agents find-or-create by name; humans need kb.official).

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_agent_auth::{AgentSubject, agent_caller};
use talaria_api_facades::kb::perms::{
    can_read, can_read_agent, granted_item_ids, granted_item_ids_for_agent,
};
use talaria_api_facades::kb::{NewSpace, create_space, list_spaces, spaces_created_in_last_day};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{optional_max_string_member, parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_perm, require_user, who_of};
use talaria_state::AppState;

use talaria_ratelimit::too_many_requests;

/// Spaces an agent may create per day. A space is org-visible by default,
/// so name variants from a looping agent are an org-wide shelf nobody asked
/// for; twenty a day is more than any honest filing needs.
const AGENT_SPACES_PER_DAY: i64 = 20;

pub(crate) fn guarded_of(
    space: &talaria_api_facades::kb::KbSpace,
) -> talaria_api_facades::kb::perms::Guarded {
    talaria_api_facades::kb::perms::Guarded {
        owner_user_id: space.owner_user_id.clone(),
        created_by: space.created_by.clone(),
        visibility: space.visibility.clone(),
        edit_policy: space.edit_policy.clone(),
    }
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    // Agents (over MCP) see org/public spaces + ones granted to them.
    let caller = agent_caller(&state.pg, &headers).await?;
    let all = match list_spaces(&state.pg).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[kb] space list failed", e)),
    };
    if let Some(caller) = caller {
        let granted = match granted_item_ids_for_agent(&state.pg, "space", &caller.model).await {
            Ok(v) => v,
            Err(e) => return Ok(internal("[kb] grant read failed", e)),
        };
        // The owner arm is the inherited-read promise: a personal assistant
        // sees the spaces its owner sees, so a private space is not hidden
        // from the person's own assistant.
        let owner = match talaria_users::assistant_owner_for(
            &state.pg,
            &AgentSubject::Caller(caller.clone()),
        )
        .await
        {
            Ok(v) => v,
            Err(e) => return Ok(internal("[kb] owner resolve failed", e)),
        };
        let spaces: Vec<_> = all
            .into_iter()
            .filter(|s| {
                granted.contains(&s.id)
                    || can_read_agent(&guarded_of(s), &caller.model, owner.as_deref(), &[], &[])
            })
            .collect();
        return Ok(Json(json!({ "spaces": spaces })).into_response());
    }
    let user = require_user(&state, &headers).await?;
    // Hide folders the caller can't read, but keep ones shared with them.
    // can_read runs with no grant list — it covers only the visibility half;
    // the granted-set check beside it covers the grant half.
    let granted = match granted_item_ids(&state.pg, "space", &user.id).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[kb] grant read failed", e)),
    };
    let who = who_of(&user);
    let spaces: Vec<_> = all
        .into_iter()
        .filter(|s| {
            granted.contains(&s.id)
                || can_read(&guarded_of(s), Some(&user.id), who.as_deref(), &[], &[])
        })
        .collect();
    Ok(Json(json!({ "spaces": spaces })).into_response())
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let name = match string_member(obj, "name", 1, 80) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let description = match optional_max_string_member(obj, "description", 400) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let icon = match optional_max_string_member(obj, "icon", 8) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Agents (over MCP) may create spaces too — a space is just a shelf, and
    // docs stay drafts until a human officializes them. The space carries
    // the agent's responsible user as its owner (a personal assistant's
    // owner, or the human an org agent is answering) so a person — not the
    // agent — governs the shelf; sharing and deletion stay human calls.
    let caller = agent_caller(&state.pg, &headers).await?;
    if let Some(caller) = caller {
        let model = caller.model.clone();
        let dup = match list_spaces(&state.pg).await {
            Ok(v) => v,
            Err(e) => return Ok(internal("[kb] space list failed", e)),
        }
        .into_iter()
        .find(|s| s.name.trim().to_lowercase() == name.trim().to_lowercase());
        if let Some(dup) = dup {
            // find-or-create: agents retry; duplicates rot the KB. But the
            // find half answers ONLY with a space this caller could have
            // listed — the same read gate GET /api/kb/spaces applies — or
            // the route hands an agent that guesses a name the full row of
            // someone's private space (its landing page, its owner). A
            // collision with a space it cannot read is a 409 and no row.
            let granted = match granted_item_ids_for_agent(&state.pg, "space", &model).await {
                Ok(v) => v,
                Err(e) => return Ok(internal("[kb] grant read failed", e)),
            };
            let owner = match talaria_users::assistant_owner_for(
                &state.pg,
                &AgentSubject::Caller(caller.clone()),
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return Ok(internal("[kb] owner resolve failed", e)),
            };
            let readable = granted.contains(&dup.id)
                || can_read_agent(&guarded_of(&dup), &model, owner.as_deref(), &[], &[]);
            if !readable {
                return Ok(house_error(
                    StatusCode::CONFLICT,
                    "a space with that name already exists; pick another name",
                ));
            }
            return Ok(Json(json!({ "space": dup })).into_response());
        }
        match spaces_created_in_last_day(&state.pg, &model).await {
            Ok(n) if n >= AGENT_SPACES_PER_DAY => {
                return Ok(too_many_requests(
                    24 * 60 * 60,
                    "this agent has created its day's share of spaces — file into an existing one",
                ));
            }
            Ok(_) => {}
            Err(e) => return Ok(internal("[kb] space count failed", e)),
        }
        let responsible = match talaria_attribution::responsible_user_for(
            &state.pg,
            state.redis().await.ok(),
            &AgentSubject::Caller(caller),
        )
        .await
        {
            Ok(v) => v,
            Err(e) => return Ok(internal("[kb] responsible-user resolve failed", e)),
        };
        let space = match create_space(
            &state.pg,
            &NewSpace {
                name,
                description,
                icon,
                created_by: model,
                owner_user_id: responsible,
            },
        )
        .await
        {
            Ok(s) => s,
            Err(e) => return Ok(internal("[kb] space create failed", e)),
        };
        return Ok(Json(json!({ "space": space })).into_response());
    }
    let user = require_perm(&state, &headers, "kb.official").await?;
    let created_by = who_of(&user).unwrap_or_else(|| "user".to_string());
    let space = match create_space(
        &state.pg,
        &NewSpace {
            name,
            description,
            icon,
            owner_user_id: Some(user.id.clone()),
            created_by,
        },
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return Ok(internal("[kb] space create failed", e)),
    };
    let (pg, actor, target_id, target_label) = (
        state.pg.clone(),
        actor_of(&user),
        space.id.clone(),
        space.name.clone(),
    );
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "kb.space_create",
                target_type: "kb-space",
                target_id: Some(&target_id),
                target_label: Some(&target_label),
                before: None,
                after: None,
            },
        )
        .await;
    });
    Ok(Json(json!({ "space": space })).into_response())
}
