// /api/workbench/env/{repo}. The per-project env store's admin wire: PATCH
// to set/delete entries (values sealed server-side, never echoed), GET for
// the key list. The VALUES never leave the database through this route —
// the only reader is the fleet render, which materializes them into the
// granted agents' containers. agents.manage, like every workbench admin
// surface.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_body::{parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_repo_env as repo_env;
use talaria_session::{actor_of, require_perm, secretbox_or_500};
use talaria_state::AppState;

fn repo_ok(repo: &str) -> bool {
    !repo.is_empty() && repo.len() <= 200 && repo.split('/').count() == 2
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(repo): Path<String>,
) -> Result<Response, Response> {
    require_perm(&state, &headers, "agents.manage").await?;
    if !repo_ok(&repo) {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "repo must be owner/name",
        ));
    }
    Ok(
        Json(json!({ "repo": repo, "keys": repo_env::env_keys(&state.pg, &repo).await }))
            .into_response(),
    )
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(repo): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_perm(&state, &headers, "agents.manage").await?;
    if !repo_ok(&repo) {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "repo must be owner/name",
        ));
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // set: [{key ≤100, value ≤10k}] — an EMPTY value is the delete affordance
    // the UI offers inline. delete: [key] for removals without re-sending.
    let mut set: Vec<(String, String)> = Vec::new();
    if let Some(v) = obj.get("set") {
        let Some(arr) = v.as_array() else {
            return Ok(house_error(StatusCode::BAD_REQUEST, "set must be an array"));
        };
        if arr.len() > 100 {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "at most 100 entries per write",
            ));
        }
        for el in arr {
            let Some(entry) = el.as_object() else {
                return Ok(house_error(
                    StatusCode::BAD_REQUEST,
                    "set entries must be objects",
                ));
            };
            let key = match string_member(entry, "key", 1, 100) {
                Ok(k) => k,
                Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
            };
            let value = match max_string_member_guard(entry, "value", 0, 10_000) {
                Ok(v) => v,
                Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
            };
            set.push((key, value));
        }
    }
    let mut delete: Vec<String> = Vec::new();
    if let Some(v) = obj.get("delete") {
        let Some(arr) = v.as_array() else {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "delete must be an array",
            ));
        };
        for el in arr {
            let Some(k) = el.as_str() else {
                return Ok(house_error(
                    StatusCode::BAD_REQUEST,
                    "delete entries must be strings",
                ));
            };
            delete.push(k.to_string());
        }
    }
    if set.is_empty() && delete.is_empty() {
        return Ok(house_error(StatusCode::BAD_REQUEST, "nothing to do"));
    }
    let sb = secretbox_or_500(&state, "[workbench/env] secretbox").await?;
    let actor = actor_of(&user);
    if let Err(e) = repo_env::patch_env(&state.pg, &sb, &repo, &actor, &set, &delete).await {
        // A validation sentence is a 400; everything else is ours.
        return Ok(if e.starts_with('"') {
            house_error(StatusCode::BAD_REQUEST, &e)
        } else {
            internal("[workbench/env] write failed", e)
        });
    }
    talaria_audit::log_audit(
        &state.pg,
        talaria_audit::AuditEntry {
            actor: &actor,
            action: "workbench.repo_env",
            target_type: "repo",
            target_id: Some(&repo),
            target_label: None,
            before: None,
            after: Some(json!({
                "set": set.iter().map(|(k, _)| k).collect::<Vec<_>>(),
                "deleted": delete,
            })),
        },
    )
    .await;
    Ok(
        Json(json!({ "repo": repo, "keys": repo_env::env_keys(&state.pg, &repo).await }))
            .into_response(),
    )
}

/// zod-style optional bounded string whose absence is None and whose null is
/// a 400 — `value` is required in every set entry (empty means delete).
fn max_string_member_guard(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    min: usize,
    max: usize,
) -> Result<String, String> {
    match obj.get(key) {
        Some(serde_json::Value::String(s)) => {
            let n = s.chars().count();
            if n < min {
                Err(talaria_body::too_small_msg(min))
            } else if n > max {
                Err(talaria_body::too_big_msg(max))
            } else {
                Ok(s.clone())
            }
        }
        Some(v) => Err(talaria_body::string_msg(talaria_body::zod_type_name(v))),
        None => Err(talaria_body::string_msg("undefined")),
    }
}
