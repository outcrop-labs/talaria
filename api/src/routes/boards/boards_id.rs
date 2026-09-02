// /api/boards/{id}. PATCH { name?, archived?, judgeMode?, teamId?, teamName? }
// → rename/archive/set the QA
// judge mode (owner/editor); a team move is owner-only because it changes who
// can see the board. DELETE → owner only. The identity here is ACTING user —
// a personal assistant patches as its owner (the identity-proxy model), and
// an elevated assistant edits any board but never at owner level.

use crate::boards::{
    archive_board, board_role, can_edit, delete_board, rename_board, set_board_judge_mode,
    set_board_team,
};
use crate::body::{
    as_object, optional_boolean_member, optional_enum_member, optional_string_member, parse,
    present_nullable_max_string_member, present_nullable_uuid_member,
};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{acting_user, require_user, unauthorized};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

const JUDGE_MODES: &[&str] = &["inherit", "off", "advisory", "enforcing"];

/// The validated Patch: each field Option<"present">, in schema order —
/// name, archived, judgeMode, teamId, teamName — which is the order the
/// validation messages answer in. teamId and teamName keep their present-null
/// state:
/// "move to personal" (null) and "don't touch" (absent) are different
/// requests, and the name arm feeds the id arm.
#[derive(Debug)]
struct Patch {
    name: Option<String>,
    archived: Option<bool>,
    judge_mode: Option<String>,
    team_id: Option<Option<String>>,
    team_name: Option<Option<String>>,
}

fn validate_patch(obj: &serde_json::Map<String, serde_json::Value>) -> Result<Patch, String> {
    Ok(Patch {
        name: optional_string_member(obj, "name", 120)?,
        archived: optional_boolean_member(obj, "archived")?,
        judge_mode: optional_enum_member(obj, "judgeMode", JUDGE_MODES)?,
        team_id: present_nullable_uuid_member(obj, "teamId")?,
        team_name: present_nullable_max_string_member(obj, "teamName", 120)?,
    })
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    // Humans, or a personal assistant acting as its owner; a legacy agent key
    // has no owner to act as and no identity of its own → the 401.
    let user = match acting_user(&state, &headers).await {
        Ok(Some(u)) => u,
        Ok(None) => return unauthorized(),
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "PATCH", &id) {
        return gate;
    }
    // An elevated assistant edits any board (never owner-level).
    let role = match board_role(&state.pg, &user.id, &id).await {
        Ok(r) => r.or_else(|| user.elevated.then(|| "editor".to_string())),
        Err(e) => {
            tracing::error!("[boards] role read on PATCH failed: {e}");
            return thrown_internal_error();
        }
    };
    if !can_edit(role.as_deref()) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let patch = match validate_patch(obj) {
        Ok(p) => p,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // Team by NAME (assistant-friendly): "" / "personal" / null → no team;
    // anything else must name one, and the miss answers with the name as the
    // human typed it, not the normalized one. Only consulted when the id arm
    // said nothing — the explicit id wins.
    let mut team_id = patch.team_id;
    if team_id.is_none()
        && let Some(raw) = &patch.team_name
    {
        let raw_str = raw.as_deref().unwrap_or("");
        let name = raw_str.trim().to_lowercase();
        let resolved = if name.is_empty() || name == "personal" {
            None
        } else {
            match sqlx::query_scalar::<_, String>(
                "select id::text from teams where lower(name) = $1",
            )
            .bind(&name)
            .fetch_optional(&state.pg)
            .await
            {
                Ok(Some(team)) => Some(team),
                Ok(None) => {
                    return house_error(
                        StatusCode::BAD_REQUEST,
                        &format!("no team named \"{raw_str}\""),
                    );
                }
                Err(e) => {
                    tracing::error!("[boards] team-by-name read failed: {e}");
                    return thrown_internal_error();
                }
            }
        };
        team_id = Some(resolved);
    }
    if let Some(target) = team_id {
        // A team move changes who can see the board — owner's call alone.
        if role.as_deref() != Some("owner") {
            return house_error(
                StatusCode::FORBIDDEN,
                "only the owner can move a board between teams",
            );
        }
        match set_board_team(&state.pg, &id, target.as_deref()).await {
            Ok(Ok(())) => {}
            // setBoardTeam's refusal is the 400 body verbatim; 'unknown
            // team' is its only in-practice message.
            Ok(Err(msg)) => return house_error(StatusCode::BAD_REQUEST, &msg),
            Err(e) => {
                tracing::error!("[boards] team move failed: {e}");
                return thrown_internal_error();
            }
        }
    }
    // The remaining writes, in the schema's order — each independent, each a
    // 500 on failure.
    if let Some(name) = &patch.name
        && let Err(e) = rename_board(&state.pg, &id, name).await
    {
        tracing::error!("[boards] rename failed: {e}");
        return thrown_internal_error();
    }
    if let Some(archived) = patch.archived
        && let Err(e) = archive_board(&state.pg, &id, archived).await
    {
        tracing::error!("[boards] archive failed: {e}");
        return thrown_internal_error();
    }
    if let Some(mode) = &patch.judge_mode
        && let Err(e) = set_board_judge_mode(&state.pg, &id, mode).await
    {
        tracing::error!("[boards] judge mode failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
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
    if let Some(gate) = crate::params::uuid_gate("boards", "DELETE", &id) {
        return gate;
    }
    let is_owner = match board_role(&state.pg, &user.id, &id).await {
        Ok(Some(role)) => role == "owner",
        Ok(None) => false,
        Err(e) => {
            tracing::error!("[boards] role read on DELETE failed: {e}");
            return thrown_internal_error();
        }
    };
    if !is_owner {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    if let Err(e) = delete_board(&state.pg, &id).await {
        tracing::error!("[boards] delete failed: {e}");
        return thrown_internal_error();
    }
    // KNOWN GAP: this delete does not purge the board's tickets + comments
    // from the activity brain. `purge_activity_by_field` exists and channel
    // delete fires its channel analog, but nothing on this path calls it, so
    // the deleted board's activity points linger in the index.
    Json(json!({ "ok": true })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn patch(v: serde_json::Value) -> Result<Patch, String> {
        validate_patch(v.as_object().unwrap())
    }

    #[test]
    fn patch_validates_in_zods_field_order() {
        // The schema order is name, archived, judgeMode, teamId, teamName —
        // two bad fields answer with the FIRST one's message.
        assert_eq!(
            patch(json!({ "archived": "yes", "name": 5 })).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        assert_eq!(
            patch(json!({ "judgeMode": "nope" })).unwrap_err(),
            "Invalid option: expected one of \"inherit\"|\"off\"|\"advisory\"|\"enforcing\""
        );
        // judgeMode is the enum whatever the JSON type.
        assert_eq!(
            patch(json!({ "judgeMode": 5 })).unwrap_err(),
            "Invalid option: expected one of \"inherit\"|\"off\"|\"advisory\"|\"enforcing\""
        );
        // archived: a present null is NOT a boolean — absent is the only skip.
        assert_eq!(
            patch(json!({ "archived": null })).unwrap_err(),
            "Invalid input: expected boolean, received null"
        );
        // teamId: uuid shape; null is a VALUE (Some(None)).
        assert_eq!(
            patch(json!({ "teamId": "not-a-uuid" })).unwrap_err(),
            "Invalid UUID"
        );
        assert_eq!(
            patch(json!({ "teamId": null })).unwrap().team_id,
            Some(None)
        );
        assert_eq!(
            patch(json!({ "teamId": "123e4567-e89b-12d3-a456-426614174000" }))
                .unwrap()
                .team_id,
            Some(Some("123e4567-e89b-12d3-a456-426614174000".into()))
        );
        assert_eq!(patch(json!({})).unwrap().team_id, None);
        // teamName: no min — the empty string is the assistant's "personal",
        // and null is present (a personal move), distinct from absent.
        let p = patch(json!({ "teamName": "" })).unwrap();
        assert_eq!(p.team_name, Some(Some("".into())));
        let p = patch(json!({ "teamName": null })).unwrap();
        assert_eq!(p.team_name, Some(None));
        assert_eq!(patch(json!({})).unwrap().team_name, None);
        // name carries its 1..120 bounds when present.
        assert_eq!(
            patch(json!({ "name": "" })).unwrap_err(),
            "Too small: expected string to have >=1 characters"
        );
    }
}
