// /api/workbench/git-hosts/{id}. One git host. GET → the redacted row with a
// live status (who the connection acts as, or why it cannot connect);
// `?installations=1` on a GitHub host lists where its App is installed (the
// easy-setup picker). PUT → change it (secrets sealed; a Forgejo host's
// workspace secret checked against the host). DELETE → remove a host nothing
// names any more. requireAdmin, like the list.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_body::{
    optional_max_string_member, optional_string_array_member, parse, string_member,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_git_hosts::registry::{self, HostPatch};
use talaria_git_hosts::{GitHost, HostRow};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;

use super::workbench_git_hosts::{GithubBody, audit, github_body};

async fn row_or_404(state: &AppState, id: &str) -> Result<HostRow, Response> {
    match registry::get_host(&state.pg, id).await {
        Ok(Some(row)) => Ok(row),
        Ok(None) => Err(house_error(StatusCode::NOT_FOUND, "no such git host")),
        Err(e) => Err(internal("[workbench/git-hosts] host read failed", e)),
    }
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    uri: Uri,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let row = row_or_404(&state, &id).await?;
    let sb = state.secretbox().await.unwrap_or_default();
    let host = talaria_git_hosts::connect(&state.pg, &sb, &row).await;
    // ?installations=… — any NON-EMPTY value counts.
    let wants_installations = uri
        .query()
        .map(|q| {
            q.split('&')
                .any(|pair| matches!(pair.strip_prefix("installations="), Some(v) if !v.is_empty()))
        })
        .unwrap_or(false);
    if wants_installations {
        let list: Vec<Value> = match host.github() {
            Some(gh) => gh
                .list_installations()
                .await
                .iter()
                .map(|(id, account)| json!({ "id": id, "account": account }))
                .collect(),
            None => Vec::new(),
        };
        return Ok(Json(json!({ "installations": list })).into_response());
    }
    let st = host.status().await;
    Ok(Json(json!({ "host": row.wire_with(&st) })).into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let bad = |msg: String| house_error(StatusCode::BAD_REQUEST, &msg);
    let name = match obj.get("name") {
        None => None,
        Some(_) => match string_member(obj, "name", 1, 80) {
            Ok(v) => Some(v),
            Err(msg) => return Ok(bad(msg)),
        },
    };
    let base_url = match obj.get("baseUrl") {
        None => None,
        Some(_) => match string_member(obj, "baseUrl", 1, 300) {
            Ok(v) => Some(v),
            Err(msg) => return Ok(bad(msg)),
        },
    };
    // Absent keeps, null or "" clears the override.
    let api_url: Option<Option<String>> = match obj.get("apiUrl") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(_) => match optional_max_string_member(obj, "apiUrl", 300) {
            Ok(v) => Some(v),
            Err(msg) => return Ok(bad(msg)),
        },
    };
    let orgs = match optional_string_array_member(obj, "repoCreationOrgs", 0, 100, 20) {
        Ok(v) => v,
        Err(msg) => return Ok(bad(msg)),
    };
    let secret: Option<Option<String>> = match obj.get("secret") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(_) => match optional_max_string_member(obj, "secret", 200) {
            Ok(v) => Some(v),
            Err(msg) => return Ok(bad(msg)),
        },
    };
    let github: Option<GithubBody> = match obj.get("github") {
        None => None,
        Some(v) => match github_body(v) {
            Ok(b) => Some(b),
            Err(msg) => return Ok(bad(msg)),
        },
    };
    let ids = github.as_ref().and_then(GithubBody::ids);
    let sb = state.secretbox().await.unwrap_or_default();
    let patch = HostPatch {
        name: name.as_deref(),
        base_url: base_url.as_deref(),
        api_url: api_url.as_ref().map(|a| a.as_deref()),
        repo_creation_orgs: orgs.as_deref(),
        secret: secret.as_ref().map(|s| s.as_deref()),
        github: github.as_ref().map(|g| g.patch(&ids)),
    };
    let updated = match registry::update_host(&state.pg, &sb, &id, &patch).await {
        Ok(Some(row)) => row,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "no such git host")),
        Err(msg) => return Ok(bad(msg)),
    };
    audit(
        &state,
        actor_of(&user),
        "workbench.git_host_update",
        updated.id.clone(),
    );
    let st = talaria_git_hosts::connect(&state.pg, &sb, &updated)
        .await
        .status()
        .await;
    Ok(Json(json!({ "host": updated.wire_with(&st) })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    match registry::delete_host(&state.pg, &id).await {
        Ok(true) => {
            audit(&state, actor_of(&user), "workbench.git_host_delete", id);
            Ok(Json(json!({ "ok": true })).into_response())
        }
        Ok(false) => Ok(house_error(StatusCode::NOT_FOUND, "no such git host")),
        // Still named by a grant, a job, a request or a plugin: the sentence
        // says what, so the admin knows what to move first.
        Err(msg) if msg.contains("still in use") => Ok(house_error(StatusCode::CONFLICT, &msg)),
        Err(e) => Ok(internal("[workbench/git-hosts] host delete failed", e)),
    }
}
