// /api/secrets/share.
//
// Sharing a working secret — and the two audiences mean two different
// things. A PERSON gets a READER grant: they can reveal it, copy it, paste
// it into their own .env, and every look is audited under their name. An
// AGENT gets a SPEND grant: it receives the handle, can pass it to any tool
// call, and can never see the value — not withheld as policy, but because
// no code path would hand it over. The asymmetry is the point; a value an
// agent can read is a value in model context and in every transcript after.
//
// OWNER ONLY, for both. A reader was let in to USE the credential; letting
// them widen the circle turns sharing into forwarding, and the person who
// put the key in no longer knows who has it.

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
    get_secret_doc, grant_secret, revoke_secret, share_secret_with, unshare_secret_from,
};

/// The union's four action literals. Same near-miss table as the folders
/// body: the `action` literal picks the branch, a wrong JSON TYPE anywhere
/// aborts to 'Invalid input', and a well-typed failure surfaces its field's
/// sentence in schema order (name's max before the id's format).
#[derive(Debug)]
enum SharePost {
    With {
        action: &'static str,
        name: String,
        user_id: String,
    },
    Agent {
        action: &'static str,
        name: String,
        agent_model: String,
    },
}

fn parse_post(obj: &serde_json::Map<String, Value>) -> Result<SharePost, String> {
    let invalid = || "Invalid input".to_string();
    let typed_str = |key: &str| -> Result<&str, String> {
        match obj.get(key) {
            Some(Value::String(s)) => Ok(s.as_str()),
            _ => Err(invalid()),
        }
    };
    // name is max-only, first in every branch's schema.
    let name_of = || -> Result<String, String> {
        let name = typed_str("name")?;
        if crate::body::utf16_len(name) > 80 {
            return Err(too_big_msg(80));
        }
        Ok(name.to_string())
    };
    match obj.get("action").and_then(|v| v.as_str()) {
        Some("share") | Some("unshare") => {
            let action = if obj["action"] == "share" {
                "share"
            } else {
                "unshare"
            };
            let name = name_of()?;
            let user_id = typed_str("userId")?;
            if !zod_uuid_ok(user_id) {
                return Err("Invalid UUID".into());
            }
            Ok(SharePost::With {
                action,
                name,
                user_id: user_id.to_string(),
            })
        }
        Some("grant") | Some("revoke") => {
            let action = if obj["action"] == "grant" {
                "grant"
            } else {
                "revoke"
            };
            let name = name_of()?;
            let agent_model = typed_str("agentModel")?;
            let n = crate::body::utf16_len(agent_model);
            if n < 1 {
                return Err(too_small_msg(1));
            }
            if n > 120 {
                return Err(too_big_msg(120));
            }
            Ok(SharePost::Agent {
                action,
                name,
                agent_model: agent_model.to_string(),
            })
        }
        _ => Err(invalid()),
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
    // The body schema is a union, and a union flattens every failure —
    // non-object bodies included — to the same two words.
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
        SharePost::With {
            action,
            name,
            user_id,
        } => {
            let ok = if action == "share" {
                share_secret_with(&state.pg, &name, &user_id, &user.id).await
            } else {
                unshare_secret_from(&state.pg, &name, &user_id, &user.id).await
            };
            let ok = match ok {
                Ok(o) => o,
                Err(e) => {
                    tracing::error!("[secrets] share failed: {e}");
                    return thrown_internal_error();
                }
            };
            if !ok {
                return house_error(StatusCode::FORBIDDEN, "not yours to share");
            }
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: if action == "share" {
                        "secrets.share"
                    } else {
                        "secrets.unshare"
                    },
                    target_type: "secret",
                    target_id: Some(&name),
                    target_label: None,
                    before: None,
                    after: Some(json!({ "userId": user_id })),
                },
            )
            .await;
            secret_response(&state.pg, &name).await
        }
        SharePost::Agent {
            action,
            name,
            agent_model,
        } => {
            // Agent side. Ownership is checked here rather than inside the
            // engine's grant, which the admin route also calls for workspace
            // credentials it owns by definition.
            let doc = match get_secret_doc(&state.pg, &name).await {
                Ok(d) => d,
                Err(e) => {
                    tracing::error!("[secrets] grant read failed: {e}");
                    return thrown_internal_error();
                }
            };
            let Some(doc) = doc else {
                return house_error(StatusCode::NOT_FOUND, "not found");
            };
            if !doc.revealable || doc.owner_user_id.as_deref() != Some(user.id.as_str()) {
                return house_error(StatusCode::FORBIDDEN, "not yours to share");
            }

            let wrote = if action == "grant" {
                grant_secret(&state.pg, &name, &agent_model, Some(&actor)).await
            } else {
                revoke_secret(&state.pg, &name, &agent_model).await
            };
            if let Err(e) = wrote {
                tracing::error!("[secrets] grant failed: {e}");
                return thrown_internal_error();
            }
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: if action == "grant" {
                        "secrets.grant"
                    } else {
                        "secrets.revoke"
                    },
                    target_type: "secret",
                    target_id: Some(&name),
                    target_label: None,
                    before: None,
                    after: Some(json!({ "agentModel": agent_model })),
                },
            )
            .await;
            secret_response(&state.pg, &name).await
        }
    }
}

async fn secret_response(pg: &sqlx::PgPool, name: &str) -> Response {
    match get_secret_doc(pg, name).await {
        Ok(doc) => Json(json!({ "secret": doc })).into_response(),
        Err(e) => {
            tracing::error!("[secrets] share re-read failed: {e}");
            thrown_internal_error()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn post(v: Value) -> Result<SharePost, String> {
        let obj = v.as_object().cloned().unwrap_or_default();
        parse_post(&obj)
    }

    const UID: &str = "1b2f3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d";

    #[test]
    fn person_and_agent_branches_parse() {
        assert!(matches!(
            post(json!({"action":"share","name":"staging-deploy-1a2b3c4d","userId":UID})).unwrap(),
            SharePost::With {
                action: "share",
                ..
            }
        ));
        assert!(matches!(
            post(json!({"action":"revoke","name":"x","agentModel":"claude-main"})).unwrap(),
            SharePost::Agent {
                action: "revoke",
                ..
            }
        ));
    }

    #[test]
    fn type_errors_abort_to_invalid_input() {
        assert_eq!(
            post(json!({"action":"share"})).unwrap_err(),
            "Invalid input"
        );
        assert_eq!(
            post(json!({"action":"grant","name":"x","agentModel":null})).unwrap_err(),
            "Invalid input"
        );
        assert_eq!(
            post(json!({"action":"move","name":"x"})).unwrap_err(),
            "Invalid input"
        );
    }

    #[test]
    fn bounds_surface_in_field_order() {
        // name's bound comes before userId's format, name before agentModel's.
        assert_eq!(
            post(json!({"action":"share","name":"n".repeat(81),"userId":"no"})).unwrap_err(),
            "Too big: expected string to have <=80 characters"
        );
        assert_eq!(
            post(json!({"action":"share","name":"n","userId":"no"})).unwrap_err(),
            "Invalid UUID"
        );
        assert_eq!(
            post(json!({"action":"grant","name":"n","agentModel":""})).unwrap_err(),
            "Too small: expected string to have >=1 characters"
        );
        assert_eq!(
            post(json!({"action":"grant","name":"n","agentModel":"a".repeat(121)})).unwrap_err(),
            "Too big: expected string to have <=120 characters"
        );
    }
}
