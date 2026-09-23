// /api/skills/marketplace — the Hermes Atlas skill catalog for the Studio's
// picker: GET ?q= (the ranked list, filtered), GET detail?repo= (one GitHub
// repo's discovered skills), POST install (write them into an owner's skill
// root). Reads are any member's (the Studio is a member surface, like the
// library it decorates); install carries the same gate the skill PUT does —
// canEdit on the destination owner.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_skills::install_skill_dir;
use talaria_body::{parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_skill_access::can_edit_skills;
use talaria_skills_marketplace::{marketplace, valid_repo};
use talaria_state::AppState;

#[derive(serde::Deserialize)]
pub struct ListQuery {
    q: Option<String>,
}

/// GET /api/skills/marketplace?q= → `{entries}`. A fetch failure is a 502
/// sentence, never an empty shelf — "no skills match" and "the catalog is
/// unreachable" are different facts and the picker renders them differently.
pub async fn get_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Response, Response> {
    require_user(&state, &headers).await?;
    let entries = marketplace()
        .entries(query.q.as_deref().unwrap_or(""))
        .await
        .map_err(|e| house_error(StatusCode::BAD_GATEWAY, &e))?;
    Ok(Json(json!({ "entries": entries })).into_response())
}

#[derive(serde::Deserialize)]
pub struct DetailQuery {
    repo: Option<String>,
}

/// GET /api/skills/marketplace/detail?repo=owner/name → the repo's skills
/// with their summaries and support-file lists (the wire's file list is
/// capped; the count is exact).
pub async fn get_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DetailQuery>,
) -> Result<Response, Response> {
    require_user(&state, &headers).await?;
    let repo = query
        .repo
        .filter(|r| valid_repo(r))
        .ok_or_else(|| house_error(StatusCode::BAD_REQUEST, "repo must be a GitHub owner/name"))?;
    let scan = marketplace()
        .scan(&repo)
        .await
        .map_err(|e| house_error(StatusCode::BAD_GATEWAY, &e))?;
    let skills: Vec<Value> = scan
        .skills
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "summary": s.summary,
                "fileCount": s.files.len(),
                "files": s.files.iter().take(24).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(Json(json!({ "repo": repo, "skills": skills })).into_response())
}

/// POST /api/skills/marketplace/install `{owner, repo, skills?}` — installs
/// the named skills (every discovered one when `skills` is absent) into the
/// owner's root. Per-skill statuses, never a clobber: a skill that already
/// exists there reports `exists` and is left exactly as it was.
pub async fn post_install(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    // Authorization BEFORE body parsing — the gate depends only on the
    // owner, and the owner is knowable only after the parse, so the
    // ordering note earns its comment: user first, owner gate second, both
    // before any fetch or write.
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let owner =
        string_member(obj, "owner", 1, 80).map_err(|m| house_error(StatusCode::BAD_REQUEST, &m))?;
    let repo =
        string_member(obj, "repo", 1, 200).map_err(|m| house_error(StatusCode::BAD_REQUEST, &m))?;
    if !valid_repo(&repo) {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "repo must be a GitHub owner/name",
        ));
    }
    // The skill picker: absent or empty = all; otherwise exact names.
    let mut pick: Option<Vec<String>> = None;
    if let Some(v) = obj.get("skills") {
        let arr = v.as_array().ok_or_else(|| {
            house_error(StatusCode::BAD_REQUEST, "skills must be an array of names")
        })?;
        if arr.len() > 100 {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "too many skills in one install",
            ));
        }
        let mut names = Vec::with_capacity(arr.len());
        for n in arr {
            let s = n.as_str().ok_or_else(|| {
                house_error(StatusCode::BAD_REQUEST, "skills must be an array of names")
            })?;
            if s.is_empty()
                || s.len() > 80
                || !s.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
                })
            {
                return Ok(house_error(
                    StatusCode::BAD_REQUEST,
                    "invalid skill name in skills",
                ));
            }
            names.push(s.to_string());
        }
        pick = Some(names);
    }

    let gate = can_edit_skills(&state.pg, &user.id, &user.role, &owner).await;
    match gate {
        Ok(true) => {}
        Ok(false) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[skills] edit gate failed", e)),
    }

    let scan = marketplace()
        .scan(&repo)
        .await
        .map_err(|e| house_error(StatusCode::BAD_GATEWAY, &e))?;
    let author = user
        .email
        .as_deref()
        .or(user.name.as_deref())
        .unwrap_or("member");

    let mut results: Vec<Value> = Vec::new();
    for skill in &scan.skills {
        if let Some(names) = &pick
            && !names.iter().any(|n| n == &skill.name)
        {
            continue;
        }
        let files: Vec<(String, Vec<u8>)> = skill
            .blobs
            .iter()
            .filter(|(rel, _)| rel.as_str() != "SKILL.md")
            .map(|(rel, blob)| (rel.clone(), blob.clone()))
            .collect();
        let status = match install_skill_dir(
            &state.pg,
            &owner,
            &skill.name,
            &skill.skill_md,
            &files,
            Some(author),
        )
        .await
        {
            Ok(true) => "installed",
            Ok(false) => "exists",
            Err(e) => return Ok(house_error(StatusCode::BAD_REQUEST, &e)),
        };
        results.push(json!({ "name": skill.name, "status": status }));
    }
    if results.is_empty() {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "none of those skills are in this repo",
        ));
    }
    Ok(
        Json(json!({ "ok": true, "owner": owner, "repo": repo, "results": results }))
            .into_response(),
    )
}
