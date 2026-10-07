// /api/me. The signed-in person's own profile: GET reads the three preference
// columns, PUT edits display name (users row + the live session, so the SPA's
// corner never waits for a re-login), preferred model (the member gate runs
// HERE, not just in the picker), platform-default reasoning effort, IANA
// zone, profile photo (`avatarUploadId` — an upload the caller made, a PNG,
// JPEG, WebP or GIF of at most 5 MB; null restores the sign-in picture) and
// status (`statusEmoji` 1–16 chars, `statusText` ≤ 100; null or blank clears)
// and job title (`title` ≤ 80, trimmed; null or blank clears). GET also
// answers `title`.
// A photo change is pushed to every one of the person's live sessions, not
// only this cookie's; status is never stored in a session — GET
// /api/auth/session reads it fresh from the users row.
//
// The photo is checked BEFORE anything is written: a refused upload (403 not
// yours, 400 wrong type or too big, 404 no such upload) changes nothing.
//
// The PUT applies its fields in sequence with no transaction: name lands
// first (DB and session), so a body carrying both a name and a refused
// model/timezone has already changed the name by the time the 403/400
// answers.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_body::{
    optional_string_member, parse, present_nullable_max_string_member,
    present_nullable_string_member, present_nullable_uuid_member,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_me::{
    gateway_models, get_prefs, is_valid_time_zone, member_model_allowlist, model_allowed_for,
    set_preferred_effort, set_preferred_model, set_timezone, set_user_name,
};
use talaria_session::{
    SessionUserWire, require_user, update_session_user, update_sessions_for_user,
};
use talaria_state::AppState;
use talaria_users::{
    avatar_refusal, avatar_upload_facts, profile_face, set_avatar_upload, set_status_emoji,
    set_status_text, set_title, user_title,
};

/// The validated PUT body — each field Option<"present">, the very thing the
/// at-least-one check counts. The nullable trio keeps present-and-null (the
/// explicit "clear") distinct from absent ("don't touch").
#[derive(Debug)]
struct MePatch {
    name: Option<String>,
    preferred_model: Option<Option<String>>,
    preferred_effort: Option<Option<String>>,
    timezone: Option<Option<String>>,
    avatar_upload_id: Option<Option<String>>,
    status_emoji: Option<Option<String>>,
    status_text: Option<Option<String>>,
    title: Option<Option<String>>,
}

/// The PUT body schema, checks in declaration order (name, preferredModel,
/// preferredEffort, timezone, avatarUploadId, statusEmoji, statusText, title — the
/// first bad field's message is the answer),
/// then the at-least-one check. Every message is pinned in the test at the
/// bottom of this file.
fn validate_me_patch(obj: &serde_json::Map<String, Value>) -> Result<MePatch, String> {
    let name = optional_string_member(obj, "name", 80)?;
    let preferred_model = present_nullable_string_member(obj, "preferredModel", 200)?;
    let preferred_effort = present_nullable_string_member(obj, "preferredEffort", 24)?;
    let timezone = present_nullable_string_member(obj, "timezone", 64)?;
    let avatar_upload_id = present_nullable_uuid_member(obj, "avatarUploadId")?;
    let status_emoji = present_nullable_string_member(obj, "statusEmoji", 16)?;
    let status_text = present_nullable_max_string_member(obj, "statusText", 100)?;
    let title = present_nullable_max_string_member(obj, "title", 80)?;
    if name.is_none()
        && preferred_model.is_none()
        && preferred_effort.is_none()
        && timezone.is_none()
        && avatar_upload_id.is_none()
        && status_emoji.is_none()
        && status_text.is_none()
        && title.is_none()
    {
        return Err("nothing to update".into());
    }
    Ok(MePatch {
        name,
        preferred_model,
        preferred_effort,
        timezone,
        avatar_upload_id,
        status_emoji,
        status_text,
        title,
    })
}

/// The stored status text (and title): trimmed, and blank means none.
fn status_text_value(raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim).filter(|t| !t.is_empty())
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let (preferred_model, preferred_effort, timezone) = match get_prefs(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[me] prefs read failed", e)),
    };
    let title = match user_title(&state.pg, &user.id).await {
        Ok(t) => t,
        Err(e) => return Ok(internal("[me] title read failed", e)),
    };
    Ok(Json(json!({
        "preferredModel": preferred_model,
        "preferredEffort": preferred_effort,
        "timezone": timezone,
        "title": title,
    }))
    .into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let patch = match validate_me_patch(obj) {
        Ok(p) => p,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    // The photo claim is decided before any write, so a refusal leaves the
    // whole profile as it was.
    if let Some(Some(upload_id)) = &patch.avatar_upload_id {
        match avatar_upload_facts(&state.pg, upload_id).await {
            Ok(Some((uploaded_by, mime, size))) => {
                if let Some(refusal) = avatar_refusal(&user.id, uploaded_by.as_deref(), &mime, size)
                {
                    let status =
                        StatusCode::from_u16(refusal.status()).unwrap_or(StatusCode::BAD_REQUEST);
                    return Ok(house_error(status, refusal.message()));
                }
            }
            Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "upload not found")),
            Err(e) => return Ok(internal("[me] avatar upload read failed", e)),
        }
    }

    let mut updated = user.clone();
    if let Some(raw) = &patch.name {
        // The bounds run on the RAW string, before the handler's trim — so
        // a spaces-only name is legal here and stores "".
        let name = raw.trim();
        if let Err(e) = set_user_name(&state.pg, &user.id, name).await {
            return Ok(internal("[me] set name failed", e));
        }
        match update_session_user(&state, &headers, &json!({ "name": name })).await {
            Ok(Some(next)) => updated = next,
            // A session that vanished mid-request keeps the auth-time user.
            Ok(None) => {}
            Err(e) => return Ok(internal("[me] session patch failed", e)),
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
                Err(e) => return Ok(internal("[me] catalog read failed", e)),
            };
            if !model_allowed_for(&user.role, model, &allow, &catalog) {
                return Ok(house_error(
                    StatusCode::FORBIDDEN,
                    "that model is not available to you — ask an admin",
                ));
            }
        }
        if let Err(e) = set_preferred_model(&state.pg, &user.id, choice.as_deref()).await {
            return Ok(internal("[me] set preferred model failed", e));
        }
    }
    if let Some(choice) = &patch.preferred_effort {
        // Deliberately NOT validated against any one model's published
        // levels: the preference travels across every model the user talks
        // to, and each surface applies it only where that model's metadata
        // vouches for the level. The length bound in the schema is the whole
        // server-side contract.
        if let Err(e) = set_preferred_effort(&state.pg, &user.id, choice.as_deref()).await {
            return Ok(internal("[me] set preferred effort failed", e));
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
                    return Ok(house_error(
                        StatusCode::BAD_REQUEST,
                        "not a recognized time zone",
                    ));
                }
                if let Err(e) = set_timezone(&state.pg, &user.id, Some(tz)).await {
                    return Ok(internal("[me] set timezone failed", e));
                }
            }
            None => {
                if let Err(e) = set_timezone(&state.pg, &user.id, None).await {
                    return Ok(internal("[me] clear timezone failed", e));
                }
            }
        }
    }
    if let Some(choice) = &patch.avatar_upload_id
        && let Err(e) = set_avatar_upload(&state.pg, &user.id, choice.as_deref()).await
    {
        return Ok(internal("[me] set avatar failed", e));
    }
    if let Some(choice) = &patch.status_emoji
        && let Err(e) = set_status_emoji(&state.pg, &user.id, choice.as_deref()).await
    {
        return Ok(internal("[me] set status emoji failed", e));
    }
    if let Some(choice) = &patch.status_text
        && let Err(e) =
            set_status_text(&state.pg, &user.id, status_text_value(choice.as_deref())).await
    {
        return Ok(internal("[me] set status text failed", e));
    }
    if let Some(choice) = &patch.title
        && let Err(e) = set_title(&state.pg, &user.id, status_text_value(choice.as_deref())).await
    {
        return Ok(internal("[me] set title failed", e));
    }

    // The face the answer (and, after a photo change, every session) carries.
    let face = match profile_face(&state.pg, &user.id).await {
        Ok(f) => f,
        Err(e) => return Ok(internal("[me] profile read failed", e)),
    };
    let (status_emoji, status_text) = match face {
        Some(face) => {
            if patch.avatar_upload_id.is_some() {
                // Every tab and device the person is signed in on, not only
                // this cookie — the corner avatar must not wait for a re-login.
                if let Err(e) =
                    update_sessions_for_user(&state, &user.id, &json!({ "picture": face.picture }))
                        .await
                {
                    return Ok(internal("[me] session picture push failed", e));
                }
            }
            updated.picture = face.picture;
            (face.status_emoji, face.status_text)
        }
        None => (None, None),
    };
    Ok(Json(json!({
        "user": SessionUserWire {
            user: updated,
            status_emoji,
            status_text,
        }
    }))
    .into_response())
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

    #[test]
    fn me_patch_takes_photo_and_status() {
        // Each new field alone is something to update.
        let p = patch(json!({ "avatarUploadId": "a1b2c3d4-0000-4000-8000-000000000001" })).unwrap();
        assert_eq!(
            p.avatar_upload_id,
            Some(Some("a1b2c3d4-0000-4000-8000-000000000001".into()))
        );
        let p = patch(json!({ "statusEmoji": "📅", "statusText": "In a meeting" })).unwrap();
        assert_eq!(p.status_emoji, Some(Some("📅".into())));
        assert_eq!(p.status_text, Some(Some("In a meeting".into())));
        // Null clears: the photo back to the provider's, the status off.
        let p = patch(json!({ "avatarUploadId": null, "statusEmoji": null, "statusText": null }))
            .unwrap();
        assert_eq!(p.avatar_upload_id, Some(None));
        assert_eq!(p.status_emoji, Some(None));
        assert_eq!(p.status_text, Some(None));
        // Bounds: emoji 1–16, text ≤ 100 (an empty text is legal — it clears).
        assert_eq!(
            patch(json!({ "statusEmoji": "x".repeat(17) })).unwrap_err(),
            "Too big: expected string to have <=16 characters"
        );
        assert_eq!(
            patch(json!({ "statusEmoji": "" })).unwrap_err(),
            "Too small: expected string to have >=1 characters"
        );
        assert!(patch(json!({ "statusEmoji": "x".repeat(16) })).is_ok());
        assert_eq!(
            patch(json!({ "statusText": "x".repeat(101) })).unwrap_err(),
            "Too big: expected string to have <=100 characters"
        );
        assert!(patch(json!({ "statusText": "x".repeat(100) })).is_ok());
        assert_eq!(
            patch(json!({ "statusText": "" })).unwrap().status_text,
            Some(Some(String::new()))
        );
        // The photo is an upload id, nothing else.
        assert_eq!(
            patch(json!({ "avatarUploadId": "not-a-uuid" })).unwrap_err(),
            "Invalid UUID"
        );
        assert_eq!(
            patch(json!({ "avatarUploadId": 5 })).unwrap_err(),
            "Invalid input: expected string, received number"
        );
    }

    #[test]
    fn me_patch_takes_a_title() {
        // Alone it is something to update; null clears; ≤ 80; blank is legal
        // (it clears, like status text).
        let p = patch(json!({ "title": "Product designer" })).unwrap();
        assert_eq!(p.title, Some(Some("Product designer".into())));
        assert_eq!(patch(json!({ "title": null })).unwrap().title, Some(None));
        assert_eq!(
            patch(json!({ "title": "" })).unwrap().title,
            Some(Some(String::new()))
        );
        assert!(patch(json!({ "title": "x".repeat(80) })).is_ok());
        assert_eq!(
            patch(json!({ "title": "x".repeat(81) })).unwrap_err(),
            "Too big: expected string to have <=80 characters"
        );
        assert_eq!(
            patch(json!({ "title": 5 })).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        // Stored trimmed; blank means no title.
        assert_eq!(status_text_value(Some("  Designer ")), Some("Designer"));
    }

    #[test]
    fn status_text_trims_and_empty_clears() {
        assert_eq!(
            status_text_value(Some("  In a meeting ")),
            Some("In a meeting")
        );
        assert_eq!(status_text_value(Some("   ")), None);
        assert_eq!(status_text_value(Some("")), None);
        assert_eq!(status_text_value(None), None);
    }
}
