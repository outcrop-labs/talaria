// /api/me. The signed-in person's own profile: GET reads the three preference
// columns, PUT edits display name (users row + the live session, so the SPA's
// corner never waits for a re-login), preferred model (the member gate runs
// HERE, not just in the picker), platform-default reasoning effort, and IANA
// zone.
//
// The PUT applies its fields in sequence with no transaction: name lands
// first (DB and session), so a body carrying both a name and a refused
// model/timezone has already changed the name by the time the 403/400
// answers.

use crate::body::{as_object, optional_string_member, parse, present_nullable_string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::me::{
    gateway_models, get_prefs, is_valid_time_zone, member_model_allowlist, model_allowed_for,
    set_preferred_effort, set_preferred_model, set_timezone, set_user_name,
};
use crate::session::{require_user, update_session_user};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// The validated PUT body — each field Option<"present">, the very thing the
/// at-least-one check counts. The nullable trio keeps present-and-null (the
/// explicit "clear") distinct from absent ("don't touch").
#[derive(Debug)]
struct MePatch {
    name: Option<String>,
    preferred_model: Option<Option<String>>,
    preferred_effort: Option<Option<String>>,
    timezone: Option<Option<String>>,
}

/// The PUT body schema, checks in declaration order (name, preferredModel,
/// preferredEffort, timezone — the first bad field's message is the answer),
/// then the at-least-one check. Every message is pinned in the test at the
/// bottom of this file.
fn validate_me_patch(obj: &serde_json::Map<String, Value>) -> Result<MePatch, String> {
    let name = optional_string_member(obj, "name", 80)?;
    let preferred_model = present_nullable_string_member(obj, "preferredModel", 200)?;
    let preferred_effort = present_nullable_string_member(obj, "preferredEffort", 24)?;
    let timezone = present_nullable_string_member(obj, "timezone", 64)?;
    if name.is_none()
        && preferred_model.is_none()
        && preferred_effort.is_none()
        && timezone.is_none()
    {
        return Err("nothing to update".into());
    }
    Ok(MePatch {
        name,
        preferred_model,
        preferred_effort,
        timezone,
    })
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let (preferred_model, preferred_effort, timezone) = match get_prefs(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[me] prefs read failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({
        "preferredModel": preferred_model,
        "preferredEffort": preferred_effort,
        "timezone": timezone,
    }))
    .into_response()
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let patch = match validate_me_patch(obj) {
        Ok(p) => p,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    let mut updated = user.clone();
    if let Some(raw) = &patch.name {
        // The bounds run on the RAW string, before the handler's trim — so
        // a spaces-only name is legal here and stores "".
        let name = raw.trim();
        if let Err(e) = set_user_name(&state.pg, &user.id, name).await {
            tracing::error!("[me] set name failed: {e}");
            return thrown_internal_error();
        }
        match update_session_user(&state, &headers, &json!({ "name": name })).await {
            Ok(Some(next)) => updated = next,
            // A session that vanished mid-request keeps the auth-time user.
            Ok(None) => {}
            Err(e) => {
                tracing::error!("[me] session patch failed: {e}");
                return thrown_internal_error();
            }
        }
    }
    if let Some(choice) = &patch.preferred_model {
        // Members may only pick allowlisted models — enforced here, not just
        // hidden in the picker (admins gate the expensive brains). Null skips
        // the gate entirely: it IS the setting "server default".
        if let Some(model) = choice {
            let allow = member_model_allowlist(&state.pg).await;
            let catalog = match gateway_models(&state.pg).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("[me] catalog read failed: {e}");
                    return thrown_internal_error();
                }
            };
            if !model_allowed_for(&user.role, model, &allow, &catalog) {
                return house_error(
                    StatusCode::FORBIDDEN,
                    "that model is not available to you — ask an admin",
                );
            }
        }
        if let Err(e) = set_preferred_model(&state.pg, &user.id, choice.as_deref()).await {
            tracing::error!("[me] set preferred model failed: {e}");
            return thrown_internal_error();
        }
    }
    if let Some(choice) = &patch.preferred_effort {
        // Deliberately NOT validated against any one model's published
        // levels: the preference travels across every model the user talks
        // to, and each surface applies it only where that model's metadata
        // vouches for the level. The length bound in the schema is the whole
        // server-side contract.
        if let Err(e) = set_preferred_effort(&state.pg, &user.id, choice.as_deref()).await {
            tracing::error!("[me] set preferred effort failed: {e}");
            return thrown_internal_error();
        }
    }
    if let Some(choice) = &patch.timezone {
        // An IANA name this runtime can resolve, or a refusal — the stored
        // value drives scheduled work, so a typo must die here. Null is the
        // setting "follow the workspace zone" and passes straight through.
        match choice {
            Some(raw) => {
                let tz = raw.trim();
                if !is_valid_time_zone(tz) {
                    return house_error(StatusCode::BAD_REQUEST, "not a recognized time zone");
                }
                if let Err(e) = set_timezone(&state.pg, &user.id, Some(tz)).await {
                    tracing::error!("[me] set timezone failed: {e}");
                    return thrown_internal_error();
                }
            }
            None => {
                if let Err(e) = set_timezone(&state.pg, &user.id, None).await {
                    tracing::error!("[me] clear timezone failed: {e}");
                    return thrown_internal_error();
                }
            }
        }
    }
    Json(json!({ "user": updated })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(v: Value) -> Result<MePatch, String> {
        validate_me_patch(v.as_object().unwrap())
    }

    #[test]
    fn me_patch_matches_the_zod_probe_table() {
        // name: optional, min 1, max 80, NOT nullable — the bounds run on the
        // raw string (a spaces-only name passes here and trims in the
        // handler).
        let ok = patch(json!({ "name": " " })).unwrap();
        assert_eq!(ok.name.as_deref(), Some(" "));
        for (bad, msg) in [
            (
                json!(""),
                "Too small: expected string to have >=1 characters",
            ),
            (
                json!("x".repeat(81)),
                "Too big: expected string to have <=80 characters",
            ),
            (json!(null), "Invalid input: expected string, received null"),
            (json!(5), "Invalid input: expected string, received number"),
            (json!([]), "Invalid input: expected string, received array"),
            (
                json!(true),
                "Invalid input: expected string, received boolean",
            ),
        ] {
            assert_eq!(
                patch(json!({ "name": bad })).unwrap_err(),
                msg,
                "name {bad:?}"
            );
        }
        // The nullable trio: null is a legal VALUE — present AND null,
        // Some(None), the explicit "clear" — and not the absent None the
        // root refine counts.
        assert_eq!(
            patch(json!({ "preferredModel": null }))
                .unwrap()
                .preferred_model,
            Some(None)
        );
        assert_eq!(
            patch(json!({ "preferredEffort": null }))
                .unwrap()
                .preferred_effort,
            Some(None)
        );
        assert_eq!(
            patch(json!({ "timezone": null })).unwrap().timezone,
            Some(None)
        );
        // The trio's shared failure rows; the per-field maxes follow.
        for field in ["preferredModel", "preferredEffort", "timezone"] {
            assert_eq!(
                patch(json!({ (field): "" })).unwrap_err(),
                "Too small: expected string to have >=1 characters",
                "{field} empty"
            );
            assert_eq!(
                patch(json!({ (field): 5 })).unwrap_err(),
                "Invalid input: expected string, received number",
                "{field} number"
            );
        }
        assert_eq!(
            patch(json!({ "preferredModel": "x".repeat(201) })).unwrap_err(),
            "Too big: expected string to have <=200 characters"
        );
        assert_eq!(
            patch(json!({ "preferredEffort": "x".repeat(25) })).unwrap_err(),
            "Too big: expected string to have <=24 characters"
        );
        assert_eq!(
            patch(json!({ "timezone": "x".repeat(65) })).unwrap_err(),
            "Too big: expected string to have <=64 characters"
        );
        // Declaration order: a bad name outranks a bad timezone.
        assert_eq!(
            patch(json!({ "name": "", "timezone": 5 })).unwrap_err(),
            "Too small: expected string to have >=1 characters"
        );
        // The at-least-one rule: nothing present (unknown keys strip)
        // answers 'nothing to update'.
        assert_eq!(patch(json!({})).unwrap_err(), "nothing to update");
        assert_eq!(
            patch(json!({ "bogus": 1 })).unwrap_err(),
            "nothing to update"
        );
    }
}
