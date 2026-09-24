// /api/fleet/defs/{id}/google — who this agent acts as on Google, and the
// connect/disconnect control for a per-agent identity.
//
// GET    → the registry row (or the legacy fallback) plus connection face
// PUT    → set principal_kind. The whole body validates before any write.
// DELETE → drop the agent's own Google connection
// GET …/connect → begin the OAuth dance that stores that connection

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_api_facades::google::agent::{
    AgentGoogleIdentity, PrincipalKind, agent_google_identity, disconnect_agent_google,
    remember_agent_oauth_state, upsert_principal,
};
use talaria_api_facades::google::client::resolve_google_client;
use talaria_api_facades::google::oauth::{
    WORKSPACE_SCOPES, google_agent_connect_redirect_uri, google_connect_url,
    google_integration_enabled, oauth_relocation,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{enum_member, parse};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, random_token, require_perm, state_cookie_for};
use talaria_state::AppState;

const KINDS: &[&str] = &["owner", "org", "agent"];

/// The whole body, validated, before any write. Owner requires a user id;
/// org and agent reject one. Unknown kind is the enum message.
pub fn parse_principal_put(
    obj: &serde_json::Map<String, Value>,
) -> Result<(PrincipalKind, Option<String>), String> {
    let kind_raw = enum_member(obj, "principalKind", KINDS)?;
    let kind = PrincipalKind::parse(&kind_raw).expect("enum_member only returns a listed kind");
    let user_id = match obj.get("principalUserId") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| "principalUserId must be a uuid or null".to_string())?;
            if !talaria_body::zod_uuid_ok(s) {
                return Err("Invalid UUID".into());
            }
            Some(s.to_string())
        }
    };
    match kind {
        PrincipalKind::Owner => {
            let id = user_id.ok_or_else(|| {
                "principalUserId is required when principalKind is owner".to_string()
            })?;
            Ok((kind, Some(id)))
        }
        PrincipalKind::Org | PrincipalKind::Agent => {
            if user_id.is_some() {
                return Err("principalUserId must be null unless principalKind is owner".into());
            }
            Ok((kind, None))
        }
    }
}

fn identity_json(id: &AgentGoogleIdentity) -> Value {
    json!({
        "principalKind": id.kind.as_str(),
        "principalUserId": id.principal_user_id,
        "connected": id.connected,
        "email": id.email,
        "legacyOwnerUserId": id.legacy_owner_user_id,
    })
}

async fn agent_model(
    state: &AppState,
    id: &str,
) -> Result<Option<(String, String, String)>, Response> {
    match sqlx::query_as("select id::text, model, display_name from agent_defs where id = $1::uuid")
        .bind(id)
        .fetch_optional(&state.pg)
        .await
    {
        Ok(row) => Ok(row),
        Err(e) => Err(internal("[fleet/defs/google] def read failed", e)),
    }
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    require_perm(&state, &headers, "agents.manage").await?;
    let Some((_, model, _)) = agent_model(&state, &id).await? else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    match agent_google_identity(&state.pg, &model).await {
        Ok(id) => Ok(Json(identity_json(&id)).into_response()),
        Err(e) => Ok(internal("[fleet/defs/google] identity read failed", e)),
    }
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_perm(&state, &headers, "agents.manage").await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // The WHOLE body validates before any write — a bad user id beside a
    // valid kind writes neither.
    let (kind, user_id) = match parse_principal_put(obj) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let Some((def_id, model, display_name)) = agent_model(&state, &id).await? else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    if let Some(uid) = user_id.as_deref() {
        let exists: Option<(i32,)> = match sqlx::query_as("select 1 from users where id = $1::uuid")
            .bind(uid)
            .fetch_optional(&state.pg)
            .await
        {
            Ok(row) => row,
            Err(e) => return Ok(internal("[fleet/defs/google] user read failed", e)),
        };
        if exists.is_none() {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "principalUserId does not name a user",
            ));
        }
    }
    if let Err(e) = upsert_principal(&state.pg, &model, kind, user_id.as_deref()).await {
        return Ok(internal("[fleet/defs/google] principal write failed", e));
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "agent.google_principal",
            target_type: "agent",
            target_id: Some(&def_id),
            target_label: Some(&display_name),
            before: None,
            after: Some(json!({
                "principalKind": kind.as_str(),
                "principalUserId": user_id,
            })),
        },
    )
    .await;
    match agent_google_identity(&state.pg, &model).await {
        Ok(id) => Ok(Json(identity_json(&id)).into_response()),
        Err(e) => Ok(internal("[fleet/defs/google] identity read failed", e)),
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_perm(&state, &headers, "agents.manage").await?;
    let Some((def_id, model, display_name)) = agent_model(&state, &id).await? else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    let restored = match disconnect_agent_google(&state.pg, &model).await {
        Ok(p) => p,
        Err(e) => return Ok(internal("[fleet/defs/google] disconnect failed", e)),
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "agent.google_disconnect",
            target_type: "agent",
            target_id: Some(&def_id),
            target_label: Some(&display_name),
            before: None,
            after: Some(json!({ "principalKind": restored.kind.as_str() })),
        },
    )
    .await;
    match agent_google_identity(&state.pg, &model).await {
        Ok(id) => Ok(Json(identity_json(&id)).into_response()),
        Err(e) => Ok(internal("[fleet/defs/google] identity read failed", e)),
    }
}

pub async fn connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    uri: Uri,
) -> Response {
    if let Err(e) = require_perm(&state, &headers, "agents.manage").await {
        return e;
    }
    let sb = state.secretbox().await.unwrap_or_default();
    if !google_integration_enabled(&state.pg, &sb).await {
        return house_error(
            StatusCode::BAD_REQUEST,
            "Google integration is not configured",
        );
    }
    let Some(cfg) = resolve_google_client(&state.pg, &sb).await else {
        return internal(
            "[fleet/defs/google]",
            "integration enabled but no client resolved",
        );
    };
    let model = match agent_model(&state, &id).await {
        Ok(Some((_, model, _))) => model,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => return e,
    };

    let public_url = talaria_auth_config::get_auth_config().public_url;
    if let Some(to) = oauth_relocation(
        public_url.as_deref(),
        &headers,
        &uri,
        &format!("/api/fleet/defs/{id}/google/connect"),
    ) {
        return (StatusCode::FOUND, [(header::LOCATION, to)]).into_response();
    }
    let state_token = random_token();
    if let Err(e) = remember_agent_oauth_state(&state.pg, &state_token, &model).await {
        return internal("[fleet/defs/google] oauth state write failed", e);
    }
    let url = google_connect_url(
        &cfg,
        &google_agent_connect_redirect_uri(public_url.as_deref(), &headers, &uri),
        &state_token,
        WORKSPACE_SCOPES,
    );
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, url),
            (header::SET_COOKIE, state_cookie_for(&headers, &state_token)),
        ],
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> serde_json::Map<String, Value> {
        v.as_object().expect("object").clone()
    }

    #[test]
    fn put_validates_the_whole_body_before_a_kind_is_usable() {
        let owner = parse_principal_put(&obj(json!({
            "principalKind": "owner",
            "principalUserId": "11111111-1111-1111-1111-111111111111",
        })))
        .expect("owner");
        assert_eq!(owner.0, PrincipalKind::Owner);
        assert!(owner.1.is_some());

        let org = parse_principal_put(&obj(json!({
            "principalKind": "org",
            "principalUserId": null,
        })))
        .expect("org");
        assert_eq!(org.0, PrincipalKind::Org);
        assert!(org.1.is_none());

        let agent = parse_principal_put(&obj(json!({ "principalKind": "agent" }))).expect("agent");
        assert_eq!(agent.0, PrincipalKind::Agent);

        assert!(
            parse_principal_put(&obj(json!({
                "principalKind": "owner",
                "principalUserId": null,
            })))
            .is_err()
        );
        assert!(
            parse_principal_put(&obj(json!({
                "principalKind": "agent",
                "principalUserId": "11111111-1111-1111-1111-111111111111",
            })))
            .is_err()
        );
        assert!(parse_principal_put(&obj(json!({ "principalKind": "nope" }))).is_err());
        assert!(
            parse_principal_put(&obj(json!({
                "principalKind": "owner",
                "principalUserId": "not-a-uuid",
            })))
            .is_err()
        );
    }
}
