// /api/secrets/folders — port of ui/src/routes/api/secrets.folders.ts.
//
// The Secrets view's own organisation, not the Files browser's. Sharing a
// folder is the point, not a bonus: a set somebody is actively working on
// gets handed to a teammate in one gesture, and the credential added next
// week is covered without anybody re-sharing — access resolves at READ time
// as the union of a secret's own grants and its folder's, never copied down
// onto rows. A PERSON shared a folder can reveal everything in it; an AGENT
// granted one can SPEND everything in it and read none of it.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::audit::{AuditEntry, log_audit};
use crate::body::{parse, too_big_msg, too_small_msg, zod_uuid_ok};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use crate::workspace_secrets::{
    FolderWho, create_secret_folder, delete_secret_folder, list_secret_folders,
    rename_secret_folder, share_secret_folder,
};

/// The four verbs, one body. The union's near-miss table (probed): the
/// `action` literal picks the branch; inside it a WRONG JSON TYPE anywhere —
/// missing, null, number where a string/boolean belongs — aborts the branch
/// and the union answers its own 'Invalid input', while a well-typed value
/// that fails a format or bound surfaces that field's sentence, in schema
/// order.
#[derive(Debug)]
enum FolderPost {
    Create {
        name: String,
    },
    Rename {
        id: String,
        name: String,
    },
    Delete {
        id: String,
    },
    Share {
        id: String,
        on: bool,
        user_id: Option<String>,
        agent_model: Option<String>,
    },
}

fn invalid_input() -> String {
    "Invalid input".into()
}

fn parse_post(obj: &serde_json::Map<String, Value>) -> Result<FolderPost, String> {
    // A required string at its type only — the bounds come after, in the
    // schema's own order.
    let typed_str = |key: &str| -> Result<&str, String> {
        match obj.get(key) {
            Some(Value::String(s)) => Ok(s.as_str()),
            _ => Err(invalid_input()),
        }
    };
    match obj.get("action").and_then(|v| v.as_str()) {
        Some("create") => {
            let name = typed_str("name")?;
            let n = crate::body::utf16_len(name);
            if n < 1 {
                return Err(too_small_msg(1));
            }
            if n > 60 {
                return Err(too_big_msg(60));
            }
            Ok(FolderPost::Create {
                name: name.to_string(),
            })
        }
        Some("rename") => {
            let id = typed_str("id")?;
            let name = typed_str("name")?;
            if !zod_uuid_ok(id) {
                return Err("Invalid UUID".into());
            }
            let n = crate::body::utf16_len(name);
            if n < 1 {
                return Err(too_small_msg(1));
            }
            if n > 60 {
                return Err(too_big_msg(60));
            }
            Ok(FolderPost::Rename {
                id: id.to_string(),
                name: name.to_string(),
            })
        }
        Some("delete") => {
            let id = typed_str("id")?;
            if !zod_uuid_ok(id) {
                return Err("Invalid UUID".into());
            }
            Ok(FolderPost::Delete { id: id.to_string() })
        }
        Some("share") => {
            let id = typed_str("id")?;
            // `on` aborts the whole branch when it is not a boolean — the
            // probe with `on: 'yes'` answers 'Invalid input' even when the
            // id is bad too.
            let on = match obj.get("on") {
                Some(Value::Bool(b)) => *b,
                _ => return Err(invalid_input()),
            };
            let user_id = match obj.get("userId") {
                None => None,
                Some(Value::String(s)) => Some(s.as_str()),
                _ => return Err(invalid_input()),
            };
            let agent_model = match obj.get("agentModel") {
                None => None,
                Some(Value::String(s)) => Some(s.as_str()),
                _ => return Err(invalid_input()),
            };
            // Format and bounds after the type gate, in schema order.
            if !zod_uuid_ok(id) {
                return Err("Invalid UUID".into());
            }
            if let Some(uid) = user_id
                && !zod_uuid_ok(uid)
            {
                return Err("Invalid UUID".into());
            }
            if let Some(am) = agent_model
                && crate::body::utf16_len(am) > 120
            {
                return Err(too_big_msg(120));
            }
            Ok(FolderPost::Share {
                id: id.to_string(),
                on,
                user_id: user_id.map(|s| s.to_string()),
                agent_model: agent_model.map(|s| s.to_string()),
            })
        }
        _ => Err(invalid_input()),
    }
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match list_secret_folders(&state.pg, &user.id, false).await {
        Ok(folders) => Json(json!({ "folders": folders })).into_response(),
        Err(e) => {
            tracing::error!("[secrets] folder list failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    // The TS schema is a zod UNION, and a union flattens EVERY failure to the
    // same two words — even a body that isn't an object at all.
    let obj = match parsed.as_object() {
        Some(o) => o,
        None => return house_error(StatusCode::BAD_REQUEST, "Invalid input"),
    };
    let action = match parse_post(obj) {
        Ok(a) => a,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let actor = actor_of(&user);

    match action {
        FolderPost::Create { name } => {
            let folder = match create_secret_folder(&state.pg, &name, Some(user.id.as_str())).await
            {
                Ok(f) => f,
                Err(e) => {
                    tracing::error!("[secrets] folder create failed: {e}");
                    return thrown_internal_error();
                }
            };
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: "secrets.folder.create",
                    target_type: "secret-folder",
                    target_id: Some(&folder.id),
                    target_label: Some(&folder.name),
                    before: None,
                    after: None,
                },
            )
            .await;
            Json(json!({ "folder": folder })).into_response()
        }
        FolderPost::Rename { id, name } => {
            let ok = match rename_secret_folder(&state.pg, &id, &name, &user.id, false).await {
                Ok(o) => o,
                Err(e) => {
                    tracing::error!("[secrets] folder rename failed: {e}");
                    return thrown_internal_error();
                }
            };
            if !ok {
                return house_error(StatusCode::FORBIDDEN, "not yours to rename");
            }
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: "secrets.folder.rename",
                    target_type: "secret-folder",
                    target_id: Some(&id),
                    target_label: Some(&name),
                    before: None,
                    after: None,
                },
            )
            .await;
            folders_response(&state.pg, &user.id).await
        }
        FolderPost::Delete { id } => {
            // The credentials survive — `on delete set null` puts them back at
            // the top level. Losing four working keys because somebody tidied
            // a label would be an unforgivable way to lose them.
            let ok = match delete_secret_folder(&state.pg, &id, &user.id, false).await {
                Ok(o) => o,
                Err(e) => {
                    tracing::error!("[secrets] folder delete failed: {e}");
                    return thrown_internal_error();
                }
            };
            if !ok {
                return house_error(StatusCode::FORBIDDEN, "not yours to delete");
            }
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: "secrets.folder.delete",
                    target_type: "secret-folder",
                    target_id: Some(&id),
                    target_label: None,
                    before: None,
                    after: None,
                },
            )
            .await;
            folders_response(&state.pg, &user.id).await
        }
        FolderPost::Share {
            id,
            on,
            user_id,
            agent_model,
        } => {
            // The TS route's truthiness ladder: a named person wins, an agent
            // named only as the empty string names nobody.
            let who = if let Some(uid) = user_id.as_deref() {
                FolderWho {
                    user_id: Some(uid.to_string()),
                    agent_model: None,
                }
            } else if let Some(am) = agent_model.as_deref().filter(|a| !a.is_empty()) {
                FolderWho {
                    user_id: None,
                    agent_model: Some(am.to_string()),
                }
            } else {
                FolderWho::default()
            };
            let ok = match share_secret_folder(&state.pg, &id, &who, on, &user.id, false).await {
                Ok(o) => o,
                Err(e) => {
                    tracing::error!("[secrets] folder share failed: {e}");
                    return thrown_internal_error();
                }
            };
            if !ok {
                return house_error(StatusCode::FORBIDDEN, "not yours to share");
            }
            let after = if let Some(uid) = &who.user_id {
                json!({ "userId": uid })
            } else if let Some(am) = &who.agent_model {
                json!({ "agentModel": am })
            } else {
                json!({})
            };
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: if on {
                        "secrets.folder.share"
                    } else {
                        "secrets.folder.unshare"
                    },
                    target_type: "secret-folder",
                    target_id: Some(&id),
                    target_label: None,
                    before: None,
                    after: Some(after),
                },
            )
            .await;
            folders_response(&state.pg, &user.id).await
        }
    }
}

async fn folders_response(pg: &sqlx::PgPool, user_id: &str) -> Response {
    match list_secret_folders(pg, user_id, false).await {
        Ok(folders) => Json(json!({ "folders": folders })).into_response(),
        Err(e) => {
            tracing::error!("[secrets] folder re-list failed: {e}");
            thrown_internal_error()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn post(v: Value) -> Result<FolderPost, String> {
        let obj = v.as_object().cloned().unwrap_or_default();
        parse_post(&obj)
    }

    #[test]
    fn action_literal_picks_the_branch() {
        assert!(matches!(
            post(json!({"action":"create","name":"Checkout"})).unwrap(),
            FolderPost::Create { ref name } if name == "Checkout"
        ));
        assert!(matches!(
            post(json!({"action":"delete","id":"1b2f3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d"})).unwrap(),
            FolderPost::Delete { .. }
        ));
    }

    #[test]
    fn unknown_or_missing_action_is_plain_invalid_input() {
        assert_eq!(
            post(json!({"action":"sharee"})).unwrap_err(),
            "Invalid input"
        );
        assert_eq!(post(json!({})).unwrap_err(), "Invalid input");
        assert_eq!(post(json!({"action":5})).unwrap_err(), "Invalid input");
        assert_eq!(post(json!({"action":null})).unwrap_err(), "Invalid input");
    }

    #[test]
    fn type_errors_abort_the_branch() {
        // `on` is not a boolean — the branch aborts, even with a bad id too.
        assert_eq!(
            post(json!({"action":"share","id":"no","on":"yes"})).unwrap_err(),
            "Invalid input"
        );
        // A missing required field is the undefined type.
        assert_eq!(
            post(json!({"action":"create"})).unwrap_err(),
            "Invalid input"
        );
        assert_eq!(
            post(json!({"action":"rename","id":"1b2f3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d"}))
                .unwrap_err(),
            "Invalid input"
        );
        // Null is a wrong type for a non-nullable optional.
        assert_eq!(
            post(json!({"action":"share","id":"1b2f3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d","on":true,"userId":null}))
                .unwrap_err(),
            "Invalid input"
        );
    }

    #[test]
    fn well_typed_failures_surface_in_field_order() {
        assert_eq!(
            post(json!({"action":"rename","id":"no","name":""})).unwrap_err(),
            "Invalid UUID"
        );
        assert_eq!(
            post(json!({"action":"rename","id":"1b2f3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d","name":""}))
                .unwrap_err(),
            "Too small: expected string to have >=1 characters"
        );
        assert_eq!(
            post(json!({"action":"create","name":"x".repeat(61)})).unwrap_err(),
            "Too big: expected string to have <=60 characters"
        );
        assert_eq!(
            post(json!({"action":"share","id":"no","on":true})).unwrap_err(),
            "Invalid UUID"
        );
    }

    #[test]
    fn share_parses_its_optionals() {
        assert!(matches!(
            post(json!({"action":"share","id":"1b2f3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d","on":false}))
                .unwrap(),
            FolderPost::Share {
                on: false,
                user_id: None,
                agent_model: None,
                ..
            }
        ));
        assert!(matches!(
            post(json!({"action":"share","id":"1b2f3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d","on":true,"agentModel":"claude-main"})).unwrap(),
            FolderPost::Share { on: true, agent_model: Some(ref am), .. } if am == "claude-main"
        ));
    }
}
