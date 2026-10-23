// /api/skills/marketplace — Hermes' official skills index for the Studio's
// picker: GET ?q= (the mirrored catalog, searched), GET detail?id=&owner=
// (one skill's files and its Skills Guard verdict — the preview), POST
// install (scan, then write it into an owner's skill root). The list reads
// the local mirror and is any member's (the Studio is a member surface, like
// the library it decorates). The two that reach GitHub and run a scan
// container carry the gate the skill PUT does — canEdit on the destination
// owner — so only someone who could install a skill can make the server
// fetch and scan one, and only for a skill the catalog lists.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_skills::{install_skill_dir, summarize_fallback};
use talaria_body::{parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{SessionUser, require_user};
use talaria_skill_access::can_edit_skills;
use talaria_skills_marketplace::{
    CatalogEntry, FetchedSkill, ScanReport, entry, fetch_skill, scan, search,
};
use talaria_state::AppState;

#[derive(serde::Deserialize)]
pub struct ListQuery {
    q: Option<String>,
}

/// GET /api/skills/marketplace?q= → `{entries}`, at most 60. A catalog that
/// cannot be read (an empty mirror whose first sync failed) is a 502
/// sentence, never an empty shelf — "no skills match" and "the catalog is
/// unreachable" are different facts and the picker renders them differently.
pub async fn get_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Response, Response> {
    require_user(&state, &headers).await?;
    let q: String = query.q.unwrap_or_default().chars().take(200).collect();
    let entries = search(&state.pg, &q)
        .await
        .map_err(|e| house_error(StatusCode::BAD_GATEWAY, &e))?;
    Ok(Json(json!({ "entries": entries })).into_response())
}

/// The catalog id the two gated routes take: Hermes' identifier, bounded.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 400 && !id.chars().any(char::is_control)
}

/// The gate both write-adjacent routes share: the signed-in user may edit
/// skills in `owner`, and `id` is a catalog row. Returns the row.
async fn gated_entry(
    state: &AppState,
    user: &SessionUser,
    owner: &str,
    id: &str,
) -> Result<CatalogEntry, Response> {
    if owner.is_empty() || owner.len() > 80 {
        return Err(house_error(StatusCode::BAD_REQUEST, "owner is required"));
    }
    if !valid_id(id) {
        return Err(house_error(
            StatusCode::BAD_REQUEST,
            "id must be a catalog identifier",
        ));
    }
    match can_edit_skills(&state.pg, &user.id, &user.role, owner).await {
        Ok(true) => {}
        Ok(false) => return Err(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Err(internal("[skills] edit gate failed", e)),
    }
    entry(&state.pg, id)
        .await
        .map_err(|e| internal("[skills] catalog read failed", e))?
        .ok_or_else(|| house_error(StatusCode::NOT_FOUND, "that skill is not in the catalog"))
}

/// Fetch, then scan, on a task of its own: a picker that stops waiting (its
/// read deadline, a closed modal) must not kill a scan halfway through a
/// cold image pull — the result lands in both caches and the next click
/// reads it from there.
async fn fetch_and_scan(
    row: CatalogEntry,
) -> Result<(Arc<FetchedSkill>, Arc<ScanReport>), Response> {
    let task = tokio::spawn(async move {
        let skill = fetch_skill(&row).await?;
        let report = scan(&skill).await.map_err(|e| {
            format!("the security scan could not run, so nothing can be installed: {e}")
        })?;
        Ok::<_, String>((skill, report))
    });
    match task.await {
        Ok(Ok(pair)) => Ok(pair),
        Ok(Err(e)) => Err(house_error(StatusCode::BAD_GATEWAY, &e)),
        Err(e) => Err(internal("[skills] marketplace fetch task died", e)),
    }
}

#[derive(serde::Deserialize)]
pub struct DetailQuery {
    id: Option<String>,
    owner: Option<String>,
}

/// GET /api/skills/marketplace/detail?id=&owner= → the skill as it would
/// install: its files (the list capped at 24; the count exact), its summary,
/// where it came from (`repo`, `commit`, `dir`, and `upstream` when an
/// official stub was followed), and `scan` — Hermes' Skills Guard verdict,
/// findings and install decision. A blocked skill is still a 200 here: the
/// preview's job is to show why.
pub async fn get_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DetailQuery>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let owner = query.owner.unwrap_or_default();
    let id = query.id.unwrap_or_default();
    let row = gated_entry(&state, &user, &owner, &id).await?;
    let (skill, report) = fetch_and_scan(row.clone()).await?;
    let files = skill.file_names();
    Ok(Json(json!({
        "id": row.id,
        "name": skill.name,
        "repo": skill.repo,
        "commit": skill.commit,
        "dir": skill.dir,
        "upstream": skill.upstream,
        "summary": summarize_fallback(&skill.skill_md),
        "fileCount": files.len(),
        "files": files.iter().take(24).collect::<Vec<_>>(),
        "scan": *report,
    }))
    .into_response())
}

/// POST /api/skills/marketplace/install `{owner, id}` — fetch the catalog
/// skill, scan it, and write it into the owner's root. Never a clobber: a
/// skill that already exists there reports `exists` and is left exactly as
/// it was. A skill Hermes' install policy blocks is a 422 carrying the scan,
/// and nothing is written; a scan that cannot run is a 502, and nothing is
/// written either.
pub async fn post_install(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    // The session first (an anonymous caller learns nothing about the body),
    // then the owner gate — both before any fetch, scan or write.
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let owner =
        string_member(obj, "owner", 1, 80).map_err(|m| house_error(StatusCode::BAD_REQUEST, &m))?;
    let id =
        string_member(obj, "id", 1, 400).map_err(|m| house_error(StatusCode::BAD_REQUEST, &m))?;
    let row = gated_entry(&state, &user, &owner, &id).await?;

    let (skill, report) = fetch_and_scan(row.clone()).await?;
    if !report.permits_install() {
        let message = format!(
            "Not installed — Hermes' Skills Guard rated {} {} ({} source, {} finding{}). {}",
            skill.name,
            report.verdict,
            report.trust_level,
            report.findings.len(),
            if report.findings.len() == 1 { "" } else { "s" },
            report.reason,
        );
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": message, "scan": *report })),
        )
            .into_response());
    }

    let author = user
        .email
        .as_deref()
        .or(user.name.as_deref())
        .unwrap_or("member");
    let files: Vec<(String, Vec<u8>)> = skill
        .files
        .iter()
        .filter(|(rel, _)| rel.as_str() != "SKILL.md")
        .cloned()
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
    Ok(Json(json!({
        "ok": true,
        "owner": owner,
        "id": row.id,
        "name": skill.name,
        "status": status,
        "scan": *report,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_are_bounded_and_printable() {
        assert!(valid_id("official/security/1password"));
        assert!(valid_id("skills-sh/timschoch/skilly/1-3-1"));
        assert!(!valid_id(""));
        assert!(!valid_id(&"a".repeat(401)));
        assert!(!valid_id("a\nb"));
    }
}
