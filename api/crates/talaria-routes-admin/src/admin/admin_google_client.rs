// /api/admin/google-client. The Google OAuth clients, by role — the
// CONNECTIONS client every Google connection (personal, org, per-agent) runs
// on, and the optional SIGN-IN client Google login runs on (login uses the
// connections client while that slot is empty). Admins register them here
// instead of editing env; secrets are SEALED and never read back.
// Deliberately requireAdmin: these are org credentials, not a grantable
// surface.
//   GET → both clients' redacted status, which client login runs on, the
//   panel's warnings, and the redirect URIs each client needs in Google Cloud
//   Console · PUT `?role=connections|signin` (default connections) → save
//   that client · DELETE `?role=…` → drop that record (env fallback resumes;
//   an empty sign-in slot hands login back to the connections client).

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::google::client::{
    ClientConfigPatch, ClientRole, GoogleClientStatus, agentic_declaration,
    clear_google_client_config, client_evidence, client_warnings, declare_agentic,
    google_client_status, google_login_enabled, google_login_pinned_by_env,
    set_google_client_config,
};
use talaria_api_facades::google::connections::get_connection_status;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    nullable_boolean_member, nullable_optional_string_member, parse, string_member,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_secretbox::SecretBox;
use talaria_session::{actor_of, require_admin, secretbox_or_500};
use talaria_state::AppState;

/// Everything the panel reads about the clients, in one place so GET and
/// every write answer with the same picture.
struct Clients {
    connections: GoogleClientStatus,
    signin: GoogleClientStatus,
    login_enabled: bool,
}

async fn read_clients(state: &AppState, sb: &SecretBox) -> Clients {
    let (connections, signin, login_enabled) = tokio::join!(
        google_client_status(&state.pg, sb, ClientRole::Connections),
        google_client_status(&state.pg, sb, ClientRole::SignIn),
        google_login_enabled(&state.pg, sb)
    );
    Clients {
        connections,
        signin,
        login_enabled,
    }
}

/// The role login runs on now: the sign-in slot's own client, else the
/// connections client, else nothing (login cannot be offered).
fn login_role(clients: &Clients) -> Option<&'static str> {
    if clients.signin.configured {
        Some(ClientRole::SignIn.as_str())
    } else if clients.connections.configured {
        Some(ClientRole::Connections.as_str())
    } else {
        None
    }
}

/// Every write's answer: the clients as they now stand.
fn answer(clients: &Clients) -> Response {
    Json(json!({
        "status": clients.connections,
        "signin": clients.signin,
        "loginRole": login_role(clients),
        "loginEnabled": clients.login_enabled,
        "loginPinnedByEnv": google_login_pinned_by_env(),
        "warnings": client_warnings(&clients.connections, &clients.signin, clients.login_enabled),
    }))
    .into_response()
}

/// Each role's list is exactly what that client must have registered: the
/// three connect callbacks on the connections client, the login callback on
/// the sign-in client — and on the connections client too while login still
/// rides it.
fn redirect_uris(origin: &str, signin_configured: bool) -> Vec<serde_json::Value> {
    let login_uri = format!("{origin}/api/auth/google/callback");
    let mut uris = vec![
        json!({ "uri": format!("{origin}/api/integrations/google/callback"), "what": "your account connect (Settings)", "role": "connections" }),
        json!({ "uri": format!("{origin}/api/integrations/google/org/callback"), "what": "org connect (Admin)", "role": "connections" }),
        json!({ "uri": format!("{origin}/api/integrations/google/agent/callback"), "what": "per-agent Google identity (agent record)", "role": "connections" }),
    ];
    if !signin_configured {
        uris.push(json!({ "uri": login_uri, "what": "Google login (only if you enable it)", "role": "connections" }));
    }
    uris.push(json!({ "uri": login_uri, "what": "Google login", "role": "signin" }));
    uris
}

/// `?role=` → the role, or the 400 sentence that names the two there are.
fn role_of(q: &HashMap<String, String>) -> Result<ClientRole, String> {
    let raw = q.get("role").map(String::as_str);
    ClientRole::parse(raw).ok_or_else(|| {
        format!(
            "Unknown client role '{}'. Use connections or signin.",
            raw.unwrap_or_default()
        )
    })
}

pub async fn get(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    uri: axum::http::Uri,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let sb = secretbox_or_500(&state, "[admin/google-client] secretbox unavailable").await?;
    let origin = talaria_auth_config::get_auth_config().public_url;
    let origin =
        talaria_api_facades::google::oauth::resolve_origin(origin.as_deref(), &headers, &uri);
    // The admin's OWN account connection — drives the "connect your account"
    // affordance in the panel (offered only once a client resolves).
    let (clients, conn) = tokio::join!(
        read_clients(&state, &sb),
        get_connection_status(&state.pg, &user.id)
    );
    let conn = match conn {
        Ok(c) => c,
        Err(e) => return Ok(internal("[admin/google-client] connection read failed", e)),
    };
    Ok(Json(json!({
        "status": clients.connections,
        "signin": clients.signin,
        "loginRole": login_role(&clients),
        "loginEnabled": clients.login_enabled,
        "loginPinnedByEnv": google_login_pinned_by_env(),
        "warnings": client_warnings(&clients.connections, &clients.signin, clients.login_enabled),
        "personalConnected": conn.connected,
        "redirectUris": redirect_uris(&origin, clients.signin.configured),
    }))
    .into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let role = match role_of(&q) {
        Ok(r) => r,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let client_id = match string_member(obj, "clientId", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let client_secret = match nullable_optional_string_member(obj, "clientSecret", 400) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let hd = match nullable_optional_string_member(obj, "hd", 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The domain restriction is the connections client's alone: login reads
    // it from there whatever the sign-in slot holds, so a domain sent for the
    // sign-in client would be a setting that does nothing.
    if role == ClientRole::SignIn && hd.as_deref().is_some_and(|h| !h.trim().is_empty()) {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "The sign-in client has no domain of its own: login uses the connections client's domain. Set it there.",
        ));
    }

    let sb = secretbox_or_500(&state, "[admin/google-client] secretbox unavailable").await?;
    let patch = ClientConfigPatch {
        client_id: Some(client_id.clone()),
        client_secret: client_secret.clone().map(Some),
        hd: match role {
            ClientRole::Connections => hd.clone().map(Some),
            ClientRole::SignIn => None,
        },
    };
    if let Err(e) = set_google_client_config(&state.pg, &sb, role, &patch).await {
        // The one refusal is a client id that trims to nothing — a 500.
        return Ok(internal("[admin/google-client] set failed", e));
    }
    // `after` carries hd only when the body's hd was a string — '' rides,
    // null/absent are dropped.
    let mut after = serde_json::Map::new();
    after.insert("clientId".into(), json!(client_id));
    after.insert(
        "secretRotated".into(),
        json!(obj.contains_key("clientSecret")),
    );
    if role == ClientRole::Connections
        && let Some(hd) = hd.as_deref()
    {
        after.insert("hd".into(), json!(hd));
    }
    let (action, target_id) = match role {
        ClientRole::Connections => ("google.client_config", "client"),
        ClientRole::SignIn => ("google.signin_client_config", "signin_client"),
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action,
            target_type: "google",
            target_id: Some(target_id),
            target_label: None,
            before: None,
            after: Some(serde_json::Value::Object(after)),
        },
    )
    .await;
    Ok(answer(&read_clients(&state, &sb).await))
}

pub async fn delete(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let role = match role_of(&q) {
        Ok(r) => r,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let sb = secretbox_or_500(&state, "[admin/google-client] secretbox unavailable").await?;
    if let Err(e) = clear_google_client_config(&state.pg, role).await {
        return Ok(internal("[admin/google-client] clear failed", e));
    }
    let (action, target_id) = match role {
        ClientRole::Connections => ("google.client_config_clear", "client"),
        ClientRole::SignIn => ("google.signin_client_config_clear", "signin_client"),
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action,
            target_type: "google",
            target_id: Some(target_id),
            target_label: None,
            before: None,
            after: None,
        },
    )
    .await;
    Ok(answer(&read_clients(&state, &sb).await))
}

// /api/admin/google-client/agentic. What the operator says Google Cloud shows
// for one client: "Use this client for an AI-powered agent" ticked (true),
// unticked (false), or not said (null). Keyed by client id, so a client moved
// between roles keeps its fact. Only a client configured here, in either
// role, takes a declaration; one pinned by AUTH_GOOGLE_CLIENT_AGENTIC /
// AUTH_GOOGLE_SIGNIN_CLIENT_AGENTIC is changed in env, not here. A
// declaration refuses nothing: the panel warns on it.

pub async fn put_agentic(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let client_id = match string_member(obj, "clientId", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Present, and true / false / null — absent is not "not said", it is a
    // body that forgot the field.
    if !obj.contains_key("agentic") {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "agentic is required: true, false, or null to withdraw the declaration.",
        ));
    }
    let agentic = match nullable_boolean_member(obj, "agentic") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    let sb = secretbox_or_500(&state, "[admin/google-client] secretbox unavailable").await?;
    let clients = read_clients(&state, &sb).await;
    let roles: Vec<&'static str> = [
        (&clients.connections, ClientRole::Connections),
        (&clients.signin, ClientRole::SignIn),
    ]
    .into_iter()
    .filter(|(s, _)| s.client_id == client_id)
    .map(|(_, r)| r.as_str())
    .collect();
    if roles.is_empty() {
        return Ok(house_error(
            StatusCode::CONFLICT,
            &format!("Client {client_id} is not configured here in either role. Reload the panel."),
        ));
    }
    let evidence = client_evidence(&state.pg).await;
    let before = agentic_declaration(&evidence, &client_id);
    if let Some(var) = before.pinned_by {
        return Ok(house_error(
            StatusCode::CONFLICT,
            &format!("{var} pins this client's declaration in the environment. Change it there."),
        ));
    }
    if let Err(e) = declare_agentic(&state.pg, &client_id, agentic).await {
        return Ok(internal("[admin/google-client/agentic] declare failed", e));
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "google.client_evidence",
            target_type: "google",
            target_id: Some(&client_id),
            target_label: Some(&roles.join(",")),
            before: Some(json!({ "declaredAgentic": before.declared })),
            after: Some(json!({ "declaredAgentic": agentic })),
        },
    )
    .await;
    Ok(answer(&read_clients(&state, &sb).await))
}

// /api/admin/google-client/login. The Google LOGIN switch — the policy half
// of the client credential (PUT /api/admin/google-client stores the
// credential; this decides whether the login screen offers it). Flipping it
// is an admin's deliberate, audit-logged act; AUTH_GOOGLE_ENABLED pinned in
// env still wins towards on.

pub async fn put_login(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let enabled = match talaria_body::boolean_member(obj, "enabled") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let sb = secretbox_or_500(&state, "[admin/google-client] secretbox unavailable").await?;
    if let Err(e) =
        talaria_api_facades::google::client::set_google_login_enabled(&state.pg, enabled).await
    {
        return Ok(internal("[admin/google-client/login] set failed", e));
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
    Ok(Json(json!({ "loginEnabled": google_login_enabled(&state.pg, &sb).await })).into_response())
}
