// /api/admin/google-client — port of ui/src/routes/api/admin.google-client.ts.
// The Google OAuth client — the credential the whole Google integration (login
// + workspace connect) runs on. Admins register it here instead of editing
// ui/.env; the secret is SEALED and never read back. Deliberately requireAdmin:
// this is an org credential, not a grantable surface.
//   GET → redacted status + the redirect URIs to register in Google Cloud
//   Console · PUT → save the client · DELETE → drop the record (env fallback
//   resumes).

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, nullable_optional_string_member, parse, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::google_client::{
    ClientConfigPatch, clear_google_client_config, google_client_status, google_login_enabled,
    google_login_pinned_by_env, set_google_client_config,
};
use crate::google_connections::get_connection_status;
use crate::secretbox::SecretBox;
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

async fn secretbox_or_500(state: &AppState) -> Result<SecretBox, Response> {
    state.secretbox().await.map_err(|e| {
        tracing::error!("[admin/google-client] secretbox unavailable: {e}");
        thrown_internal_error()
    })
}

pub async fn get(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    uri: axum::http::Uri,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let sb = match secretbox_or_500(&state).await {
        Ok(sb) => sb,
        Err(res) => return res,
    };
    let origin = crate::auth_config::get_auth_config().public_url;
    let origin = crate::google_oauth::resolve_origin(origin.as_deref(), &headers, &uri);
    // The admin's OWN account connection — drives the "connect your account"
    // affordance in the panel (offered only once a client resolves).
    let (status, login_enabled, conn) = tokio::join!(
        google_client_status(&state.pg, &sb),
        google_login_enabled(&state.pg, &sb),
        get_connection_status(&state.pg, &user.id)
    );
    let conn = match conn {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[admin/google-client] connection read failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({
        "status": status,
        "loginEnabled": login_enabled,
        "loginPinnedByEnv": google_login_pinned_by_env(),
        "personalConnected": conn.connected,
        "redirectUris": [
            { "uri": format!("{origin}/api/integrations/google/callback"), "what": "your account connect (Settings)" },
            { "uri": format!("{origin}/api/integrations/google/org/callback"), "what": "org connect (Admin)" },
            { "uri": format!("{origin}/api/auth/google/callback"), "what": "Google login (only if you enable it)" },
        ],
    }))
    .into_response()
}

pub async fn put(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let client_id = match string_member(obj, "clientId", 1, 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let client_secret = match nullable_optional_string_member(obj, "clientSecret", 400) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let hd = match nullable_optional_string_member(obj, "hd", 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    let sb = match secretbox_or_500(&state).await {
        Ok(sb) => sb,
        Err(res) => return res,
    };
    let patch = ClientConfigPatch {
        client_id: Some(client_id.clone()),
        client_secret: client_secret.clone().map(Some),
        hd: hd.clone().map(Some),
    };
    if let Err(e) = set_google_client_config(&state.pg, &sb, &patch).await {
        // The one refusal is a client id that trims to nothing. TS lets the
        // throw escape the route (no catch, no boundary) and the server
        // answers an unstructured 500 — same landing, fixed sentence.
        tracing::error!("[admin/google-client] set failed: {e}");
        return thrown_internal_error();
    }
    // `after` carries hd only when the body's hd was a string ('' rides,
    // null/absent are dropped by JSON.stringify).
    let mut after = serde_json::Map::new();
    after.insert("clientId".into(), json!(client_id));
    after.insert(
        "secretRotated".into(),
        json!(obj.contains_key("clientSecret")),
    );
    if let Some(hd) = hd.as_deref() {
        after.insert("hd".into(), json!(hd));
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "google.client_config",
            target_type: "google",
            target_id: Some("client"),
            target_label: None,
            before: None,
            after: Some(serde_json::Value::Object(after)),
        },
    )
    .await;
    let (status, login_enabled) = tokio::join!(
        google_client_status(&state.pg, &sb),
        google_login_enabled(&state.pg, &sb)
    );
    Json(json!({
        "status": status,
        "loginEnabled": login_enabled,
        "loginPinnedByEnv": google_login_pinned_by_env(),
    }))
    .into_response()
}

pub async fn delete(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let sb = match secretbox_or_500(&state).await {
        Ok(sb) => sb,
        Err(res) => return res,
    };
    if let Err(e) = clear_google_client_config(&state.pg).await {
        tracing::error!("[admin/google-client] clear failed: {e}");
        return thrown_internal_error();
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "google.client_config_clear",
            target_type: "google",
            target_id: Some("client"),
            target_label: None,
            before: None,
            after: None,
        },
    )
    .await;
    let (status, login_enabled) = tokio::join!(
        google_client_status(&state.pg, &sb),
        google_login_enabled(&state.pg, &sb)
    );
    Json(json!({
        "status": status,
        "loginEnabled": login_enabled,
        "loginPinnedByEnv": google_login_pinned_by_env(),
    }))
    .into_response()
}

// /api/admin/google-client/login — port of admin.google-client.login.ts. The
// Google LOGIN switch — the policy half of the client credential (PUT
// /api/admin/google-client stores the credential; this decides whether the
// login screen offers it). Flipping it is an admin's deliberate, audit-logged
// act; AUTH_GOOGLE_ENABLED pinned in env still wins towards on.

pub async fn put_login(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let enabled = match crate::body::boolean_member(obj, "enabled") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let sb = match secretbox_or_500(&state).await {
        Ok(sb) => sb,
        Err(res) => return res,
    };
    if let Err(e) = crate::google_client::set_google_login_enabled(&state.pg, enabled).await {
        tracing::error!("[admin/google-client/login] set failed: {e}");
        return thrown_internal_error();
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "google.login_config",
            target_type: "google",
            target_id: Some("login"),
            target_label: None,
            before: None,
            after: Some(json!({ "enabled": enabled })),
        },
    )
    .await;
    Json(json!({ "loginEnabled": google_login_enabled(&state.pg, &sb).await })).into_response()
}
