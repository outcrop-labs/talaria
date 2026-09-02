// /api/workbench/github — port of ui/src/routes/api/workbench.github.ts. The
// Workbench's GitHub connection. Deliberately requireAdmin (not
// agents.manage): this holds ORG CREDENTIALS (PAT / App private key) — a
// grantable permission shouldn't reach them. GET → live-verified redacted
// status (+ ?installations=… lists where the App is installed, the
// easy-setup picker); PUT → patch config (secrets sealed); DELETE →
// disconnect.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    as_object, object_msg, optional_enum_member, optional_max_string_member,
    optional_string_array_member, parse, present_nullable_max_string_member, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::github as gh;
use crate::github::PatchField;
use crate::session::{actor_of, require_admin};
use crate::state::AppState;

/// `z.enum(['app','pat']).nullable().optional()` — the tri-state: absent is
/// "don't touch" (None), present-null is "clear it" (Some(None)), a present
/// value must be one of the options.
fn present_nullable_enum_member(
    obj: &Map<String, Value>,
    key: &str,
    options: &[&str],
) -> Result<Option<Option<String>>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(_) => optional_enum_member(obj, key, options).map(Some),
    }
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    // searchParams.get('installations') — any NON-EMPTY value is truthy; the
    // bare `?installations` and `?installations=` are '' and fall through.
    let wants_installations = uri
        .query()
        .map(|q| {
            q.split('&')
                .any(|pair| matches!(pair.strip_prefix("installations="), Some(v) if !v.is_empty()))
        })
        .unwrap_or(false);
    let sb = state.secretbox().await.unwrap_or_default();
    if wants_installations {
        let installations = gh::list_installations(&state.pg, &sb).await;
        let list: Vec<_> = installations
            .iter()
            .map(|(id, account)| json!({ "id": id, "account": account }))
            .collect();
        return Json(json!({ "installations": list })).into_response();
    }
    let status = gh::github_status(&state.pg, &sb).await;
    Json(json!({ "status": status })).into_response()
}

pub async fn put(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // The Body schema → the engine's tri-state patch, field by field in
    // schema order. `pat` and `app` are optional OBJECTS — present-but-wrong
    // type answers zod's object message.
    let mode = match present_nullable_enum_member(obj, "mode", &["app", "pat"]) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let pat_token = match obj.get("pat") {
        None => None,
        Some(v) => {
            let Some(m) = v.as_object() else {
                return house_error(StatusCode::BAD_REQUEST, &object_msg(zod_type_name(v)));
            };
            match present_nullable_max_string_member(m, "token", 400) {
                Ok(t) => t,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            }
        }
    };
    let (app_id, installation_ids, private_key) = match obj.get("app") {
        None => (None, None, None),
        Some(v) => {
            let Some(m) = v.as_object() else {
                return house_error(StatusCode::BAD_REQUEST, &object_msg(zod_type_name(v)));
            };
            let app_id = match optional_max_string_member(m, "appId", 40) {
                Ok(v) => v,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            let installation_ids =
                match optional_string_array_member(m, "installationIds", 0, 40, 20) {
                    Ok(v) => v,
                    Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
                };
            let private_key = match present_nullable_max_string_member(m, "privateKey", 20_000) {
                Ok(v) => v,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            (app_id, installation_ids, private_key)
        }
    };
    let repo_creation_orgs = match optional_string_array_member(obj, "repoCreationOrgs", 1, 100, 10)
    {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // The slice fields borrow: keep the owned Vecs alive beside the patch.
    let installation_refs: Option<Vec<&str>> = installation_ids
        .as_ref()
        .map(|v| v.iter().map(String::as_str).collect());
    let org_refs: Option<Vec<&str>> = repo_creation_orgs
        .as_ref()
        .map(|v| v.iter().map(String::as_str).collect());
    let patch = gh::GithubConfigPatch {
        mode: match &mode {
            None => PatchField::Unset,
            Some(None) => PatchField::Set(None),
            Some(Some(m)) => PatchField::Set(Some(m.as_str())),
        },
        pat_token: match &pat_token {
            None => PatchField::Unset,
            Some(None) => PatchField::Set(None),
            Some(Some(t)) => PatchField::Set(Some(t.as_str())),
        },
        app_id: app_id.as_deref(),
        installation_ids: installation_refs.as_deref(),
        private_key: match &private_key {
            None => PatchField::Unset,
            Some(None) => PatchField::Set(None),
            Some(Some(k)) => PatchField::Set(Some(k.as_str())),
        },
        repo_creation_orgs: org_refs.as_deref(),
    };
    let sb = state.secretbox().await.unwrap_or_default();
    if let Err(e) = gh::set_github_config(&state.pg, &sb, &patch).await {
        tracing::error!("[workbench/github] config write failed: {e}");
        return thrown_internal_error();
    }
    let pg = state.pg.clone();
    let actor = actor_of(&user);
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "workbench.github",
                target_type: "workbench",
                target_id: Some("github"),
                target_label: None,
                before: None,
                after: None,
            },
        )
        .await;
    });
    let status = gh::github_status(&state.pg, &sb).await;
    Json(json!({ "status": status })).into_response()
}

pub async fn delete(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let sb = state.secretbox().await.unwrap_or_default();
    let cur = gh::get_github_config(&state.pg).await;
    // The full clear: mode off, PAT gone, App identity blanked. repoCreation-
    // Orgs rides absent (unchanged). Secrets unseal (never leave) only to be
    // overwritten with nothing.
    let no_ids: [&str; 0] = [];
    let clear = gh::GithubConfigPatch {
        mode: PatchField::Set(None),
        pat_token: PatchField::Set(None),
        app_id: Some(""),
        installation_ids: Some(&no_ids),
        private_key: PatchField::Set(None),
        repo_creation_orgs: None,
    };
    if let Err(e) = gh::set_github_config(&state.pg, &sb, &clear).await {
        tracing::error!("[workbench/github] disconnect failed: {e}");
        return thrown_internal_error();
    }
    let target_id = cur.mode.unwrap_or_else(|| "none".to_string());
    let pg = state.pg.clone();
    let actor = actor_of(&user);
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "workbench.github_disconnect",
                target_type: "workbench",
                target_id: Some(&target_id),
                target_label: None,
                before: None,
                after: None,
            },
        )
        .await;
    });
    Json(json!({ "ok": true })).into_response()
}

#[cfg(test)]
mod tests {
    use crate::body::{array_too_big_msg, enum_msg, too_big_msg};

    use super::*;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn enum_member_tri_state() {
        let mut m = Map::new();
        assert!(
            present_nullable_enum_member(&m, "mode", &["app", "pat"])
                .unwrap()
                .is_none()
        );
        m.insert("mode".into(), Value::Null);
        assert_eq!(
            present_nullable_enum_member(&m, "mode", &["app", "pat"]).unwrap(),
            Some(None)
        );
        m.insert("mode".into(), json!("pat"));
        assert_eq!(
            present_nullable_enum_member(&m, "mode", &["app", "pat"]).unwrap(),
            Some(Some("pat".into()))
        );
        m.insert("mode".into(), json!("oauth"));
        assert_eq!(
            present_nullable_enum_member(&m, "mode", &["app", "pat"]).unwrap_err(),
            enum_msg(&["app", "pat"])
        );
        m.insert("mode".into(), json!(3));
        assert_eq!(
            present_nullable_enum_member(&m, "mode", &["app", "pat"]).unwrap_err(),
            enum_msg(&["app", "pat"])
        );
    }

    #[test]
    fn tri_state_reader_null_vs_absent() {
        let mut m = Map::new();
        assert!(
            present_nullable_max_string_member(&m, "token", 400)
                .unwrap()
                .is_none()
        );
        m.insert("token".into(), Value::Null);
        assert_eq!(
            present_nullable_max_string_member(&m, "token", 400).unwrap(),
            Some(None)
        );
        m.insert("token".into(), json!("ghp_x"));
        assert_eq!(
            present_nullable_max_string_member(&m, "token", 400).unwrap(),
            Some(Some("ghp_x".into()))
        );
        // "" is a legal VALUE for this shape (max-only, no min) — the engine
        // decides what an empty token means, not the schema.
        m.insert("token".into(), json!(""));
        assert_eq!(
            present_nullable_max_string_member(&m, "token", 400).unwrap(),
            Some(Some(String::new()))
        );
    }

    #[test]
    fn token_cap_is_400() {
        let long = "x".repeat(401);
        let m = obj(json!({ "token": long }));
        assert_eq!(
            present_nullable_max_string_member(&m, "token", 400).unwrap_err(),
            too_big_msg(400)
        );
    }

    #[test]
    fn installation_ids_capped_at_20() {
        let ids: Vec<Value> = (0..21).map(|i| json!(i.to_string())).collect();
        let m = obj(json!({ "installationIds": ids }));
        assert_eq!(
            optional_string_array_member(&m, "installationIds", 0, 40, 20).unwrap_err(),
            array_too_big_msg(20)
        );
    }
}
