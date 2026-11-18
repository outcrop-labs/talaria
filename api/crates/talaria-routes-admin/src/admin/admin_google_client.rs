// /api/admin/google-client. The Google OAuth clients, by role — the
// CONNECTIONS client every Google connection (personal, org, per-agent) runs
// on, and the optional SIGN-IN client Google login runs on (login uses the
// connections client while that slot is empty). Admins register them here
// instead of editing env; secrets are SEALED and never read back.
// Deliberately requireAdmin: these are org credentials, not a grantable
// surface.
//   GET → both clients' redacted status (each with its agentic declaration
//   and Google's answer to the last check, `lastCheck`), which client login
//   runs on, the
//   panel's warnings, the redirect URIs each client needs in Google Cloud
//   Console, the client that made the admin's own connection
//   (`personalClientId`: Check with Google runs on that client alone), and
//   the census: every stored grant counted per OAuth client
//   that minted it (personal, org, agent; how many carry a refusal), with
//   the slot that client sits in now — `shadowed` when it is only the env
//   pair behind that slot's Admin record (it refreshes what it minted and
//   mints nothing new), null when it is configured nowhere, and a null
//   clientId for grants not yet recorded · PUT
//   `?role=connections|signin` (default connections) → save that client; a
//   new secret forgets the refusals recorded on that client's grants · DELETE
//   `?role=…` → drop that record (env fallback resumes;
//   an empty sign-in slot hands login back to the connections client, and
//   with no connections client and no AUTH_GOOGLE_HD login stops, sign-in
//   client or not — login never runs without the connections side's domain).

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_auth::{epoch_ms_to_iso, now_ms};
use talaria_api_facades::google::client::{
    ClientCheck, ClientConfigPatch, ClientRole, GoogleClient, GoogleClientStatus,
    agentic_declaration, clear_google_client_config, client_evidence, client_warnings,
    configured_google_clients, declare_agentic, google_client_status, google_login_enabled,
    google_login_pinned_by_env, record_check, resolve_google_signin_client,
    set_google_client_config,
};
use talaria_api_facades::google::connections::{
    ClientCensus, GrantRow, clear_client_flags, get_connection_status, grant_census, vend_grant,
};
use talaria_api_facades::google::errors::google_token_fail;
use talaria_api_facades::google::mcp::{CHECK_TOOL, GMAIL, check_agentic};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    nullable_boolean_member, nullable_optional_string_member, parse, string_member,
};
use talaria_error::{house_error, house_error_msg, internal, object_or_400};
use talaria_secretbox::SecretBox;
use talaria_session::{actor_of, require_admin, secretbox_or_500};
use talaria_state::AppState;

/// Everything the panel reads about the clients, in one place so GET and
/// every write answer with the same picture.
struct Clients {
    connections: GoogleClientStatus,
    signin: GoogleClientStatus,
    /// The slot login's client came from, as the login resolver itself
    /// answers — None when login has no client to run on.
    login_role: Option<ClientRole>,
    login_enabled: bool,
}

async fn read_clients(state: &AppState, sb: &SecretBox) -> Clients {
    let (connections, signin, login_client, login_enabled) = tokio::join!(
        google_client_status(&state.pg, sb, ClientRole::Connections),
        google_client_status(&state.pg, sb, ClientRole::SignIn),
        resolve_google_signin_client(&state.pg, sb),
        google_login_enabled(&state.pg, sb)
    );
    Clients {
        connections,
        signin,
        login_role: login_client.map(|c| c.role),
        login_enabled,
    }
}

/// The role login runs on now: the sign-in slot's own client, else the
/// connections client, else nothing (login cannot be offered — which
/// includes a sign-in client with no connections client and no domain
/// written down to take its domain from).
fn login_role(clients: &Clients) -> Option<&'static str> {
    clients.login_role.map(ClientRole::as_str)
}

/// Every write's answer: the clients as they now stand.
/// Every write's answer. Check with Google answers the same keys plus its
/// `check`, spelled out there: the generated reference reads a route's
/// shape from its `Json(json!({…}))` literal.
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

/// The census's two reads: live grants per minting client, and every client
/// a stored grant can refresh with.
async fn census_reads(
    state: &AppState,
    sb: &SecretBox,
) -> Result<(Vec<ClientCensus>, Vec<GoogleClient>), sqlx::Error> {
    let (rows, configured) = tokio::join!(
        grant_census(&state.pg),
        configured_google_clients(&state.pg, sb)
    );
    Ok((rows?, configured))
}

/// Where a census row's client sits now, as (role, source, shadowed): the
/// slot whose own client it is (`active`: each slot's configured client id
/// and source), else the env pair an Admin record shadows in a slot
/// (`shadowed` — it refreshes the grants it minted and mints none), else
/// None: configured nowhere, so its grants answer a named reconnect.
fn census_slot(
    client_id: &str,
    active: &[(ClientRole, &str, &'static str)],
    configured: &[GoogleClient],
) -> Option<(&'static str, &'static str, bool)> {
    if let Some(&(role, _, source)) = active.iter().find(|(_, id, _)| *id == client_id) {
        return Some((role.as_str(), source, false));
    }
    configured
        .iter()
        .find(|k| k.client_id == client_id)
        .map(|k| (k.role.as_str(), k.source, true))
}

/// Stored grants per minting client, each with the slot that client sits in
/// now (`census_slot`). The census an operator reads before and after moving
/// clients between slots.
fn census(
    rows: Vec<ClientCensus>,
    configured: &[GoogleClient],
    clients: &Clients,
) -> Vec<serde_json::Value> {
    let active: Vec<(ClientRole, &str, &'static str)> = [
        (ClientRole::Connections, &clients.connections),
        (ClientRole::SignIn, &clients.signin),
    ]
    .into_iter()
    .filter(|(_, s)| s.configured)
    .map(|(role, s)| (role, s.client_id.as_str(), s.source.unwrap_or_default()))
    .collect();
    rows.into_iter()
        .map(|c| {
            let slot = c
                .client_id
                .as_deref()
                .and_then(|id| census_slot(id, &active, configured));
            json!({
                "clientId": c.client_id,
                "role": slot.map(|(role, _, _)| role),
                "source": slot.map(|(_, source, _)| source),
                "shadowed": slot.is_some_and(|(_, _, shadowed)| shadowed),
                "personal": c.personal,
                "org": c.org,
                "agent": c.agent,
                "flagged": c.flagged,
            })
        })
        .collect()
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
    let (clients, conn, reads) = tokio::join!(
        read_clients(&state, &sb),
        get_connection_status(&state.pg, &user.id),
        census_reads(&state, &sb)
    );
    let conn = match conn {
        Ok(c) => c,
        Err(e) => return Ok(internal("[admin/google-client] connection read failed", e)),
    };
    let census = match reads {
        Ok((rows, configured)) => census(rows, &configured, &clients),
        Err(e) => return Ok(internal("[admin/google-client] census read failed", e)),
    };
    Ok(Json(json!({
        "status": clients.connections,
        "signin": clients.signin,
        "loginRole": login_role(&clients),
        "loginEnabled": clients.login_enabled,
        "loginPinnedByEnv": google_login_pinned_by_env(),
        "warnings": client_warnings(&clients.connections, &clients.signin, clients.login_enabled),
        "personalConnected": conn.connected,
        // The client that made the admin's own connection: the one card
        // whose "Check with Google" can run (it calls with this connection).
        "personalClientId": conn.client_id,
        "redirectUris": redirect_uris(&origin, clients.signin.configured),
        "census": census,
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
    // A new secret may be the fix for a refusal recorded on this client's
    // grants: forget those, and let the next refresh ask Google again.
    if client_secret
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
        && let Err(e) = clear_client_flags(&state.pg, client_id.trim()).await
    {
        tracing::error!("[admin/google-client] clearing refusal flags failed: {e}");
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
// declaration refuses no REST call: the panel warns on it. A client declared
// unticked keeps its grants off Google's Workspace MCP servers (the
// agent-token door) unless Google's last answer to Check with Google
// accepted a call.

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

// /api/admin/google-client/check. "Check with Google": the one
// provider-side evidence of a client's agentic mark. Google's Workspace MCP
// servers reject a client not marked "Use this client for an AI-powered
// agent", so this makes one `tools/call list_labels` to Gmail's MCP server
// (one unit of its query-cost quota) with the calling admin's OWN Google
// connection — which must have been made with the client being checked — and
// stores Google's answer under that client's id (`lastCheck`): the HTTP
// status and Google's words verbatim, never a class read from them. An
// accepted call verifies the mark. A refusal is not read as "not marked"
// (the same call fails, in Google's words, for a project not enrolled in the
// Developer Preview or without the Gmail MCP service enabled), but it is the
// last answer: it replaces an earlier acceptance, and the client's standing
// falls back to its declaration. The connection's recorded client is read
// before its token: no connection, or one recorded as made with another
// client, answers 409 with nothing sent to Google, not even a refresh. One
// that has not recorded its client is vended, since its refresh records it.
// `?role=`
// names the slot whose own client is checked. A sign-in client that has only
// ever run login made no stored connection and cannot be checked this way —
// nor needs to be: login is not an agent. One that was the connections client
// before it moved to the sign-in slot can, through a connection it made then
// (the grant keeps its minting client). A token that cannot be had
// answers as the agent routes do (`google_token_fail`); a call that never
// got Google's answer records nothing.

pub async fn post_check(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let role = match role_of(&q) {
        Ok(r) => r,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let sb = secretbox_or_500(&state, "[admin/google-client/check] secretbox unavailable").await?;
    let clients = read_clients(&state, &sb).await;
    let slot = match role {
        ClientRole::Connections => &clients.connections,
        ClientRole::SignIn => &clients.signin,
    };
    if !slot.configured {
        return Ok(house_error_msg(
            StatusCode::CONFLICT,
            "no_client",
            &match role {
                ClientRole::Connections => {
                    "There is no connections client to check. Save one first.".to_string()
                }
                ClientRole::SignIn => "There is no sign-in client to check: login uses the connections client, so check that one.".to_string(),
            },
        ));
    }
    let client_id = slot.client_id.clone();
    let not_connected = || {
        house_error_msg(
            StatusCode::CONFLICT,
            "not_connected",
            &format!(
                "Check with Google calls Gmail's MCP server with your own Google connection, made with client {client_id}, and you have no Google connection. Connect your account in Settings \u{2192} Connections, then check again."
            ),
        )
    };
    // The stored connection first: none, or one recorded as made with
    // another client, is refused before its token is vended, so nothing
    // reaches Google. One that has not recorded its client is vended: the
    // refresh is what records it.
    let stored = match get_connection_status(&state.pg, &user.id).await {
        Ok(c) => c,
        Err(e) => {
            return Ok(internal(
                "[admin/google-client/check] connection read failed",
                e,
            ));
        }
    };
    if !stored.connected {
        return Ok(not_connected());
    }
    if stored.client_id.as_deref().is_some_and(|s| !s.is_empty())
        && let Some(refusal) = speaks_for(stored.client_id.as_deref(), &client_id)
    {
        return Ok(refusal);
    }
    let vended = match vend_grant(&state.pg, &sb, &GrantRow::User(user.id.clone()), now_ms()).await
    {
        Ok(Some(v)) => v,
        Ok(None) => return Ok(not_connected()),
        Err(e) => return Ok(google_token_fail(e, "Gmail")),
    };
    // The vend's client: the one a refresh just recorded, or a reconnect's
    // between the two reads.
    if let Some(refusal) = speaks_for(vended.client_id.as_deref(), &client_id) {
        return Ok(refusal);
    }
    let answer = match check_agentic(&vended.token).await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("[admin/google-client/check] {client_id}: {e}");
            return Ok(house_error_msg(
                StatusCode::BAD_GATEWAY,
                "google_unreachable",
                &format!("Google's answer did not arrive: {e}. Nothing was recorded; check again."),
            ));
        }
    };
    let check = ClientCheck {
        ok: answer.ok,
        status: answer.status,
        text: answer.text,
        at: epoch_ms_to_iso(now_ms()),
    };
    if let Err(e) = record_check(&state.pg, &client_id, &check).await {
        return Ok(internal("[admin/google-client/check] record failed", e));
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "google.client_check",
            target_type: "google",
            target_id: Some(&client_id),
            target_label: Some(role.as_str()),
            before: None,
            after: Some(json!({
                "ok": check.ok,
                "status": check.status,
                "server": GMAIL.endpoint,
                "tool": CHECK_TOOL,
            })),
        },
    )
    .await;
    // `answer`'s keys plus the check (see `answer` for why it is a literal).
    let clients = read_clients(&state, &sb).await;
    Ok(Json(json!({
        "status": clients.connections,
        "signin": clients.signin,
        "loginRole": login_role(&clients),
        "loginEnabled": clients.login_enabled,
        "loginPinnedByEnv": google_login_pinned_by_env(),
        "warnings": client_warnings(&clients.connections, &clients.signin, clients.login_enabled),
        "check": check,
    }))
    .into_response())
}

/// None when a connection minted by `minted_by` can speak for `client_id`;
/// else the 409 that says why — another client made it, or it has not
/// recorded its client yet.
fn speaks_for(minted_by: Option<&str>, client_id: &str) -> Option<Response> {
    match minted_by.filter(|s| !s.is_empty()) {
        Some(id) if id == client_id => None,
        Some(other) => Some(house_error_msg(
            StatusCode::CONFLICT,
            "other_client",
            &format!(
                "Your own Google connection was made with client {other}, not {client_id}, so it cannot speak for {client_id}. Check with Google uses a connection made with the client being checked: check {other} instead, or reconnect your account while {client_id} is the connections client."
            ),
        )),
        None => Some(house_error_msg(
            StatusCode::CONFLICT,
            "client_unrecorded",
            "Your own Google connection has not recorded which client made it yet. It does on its next refresh, within the hour; check again then.",
        )),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn client(id: &str, role: ClientRole, source: &'static str) -> GoogleClient {
        GoogleClient {
            client_id: id.into(),
            client_secret: String::new(),
            hd: None,
            source,
            role,
        }
    }

    #[test]
    fn the_census_names_a_shadowed_env_pair_apart_from_the_slots_own_client() {
        // Connections record conn-a over env pair conn-env; sign-in record
        // signin-b. `configured_google_clients`' order.
        let configured = [
            client("conn-a", ClientRole::Connections, "admin"),
            client("conn-env", ClientRole::Connections, "env"),
            client("signin-b", ClientRole::SignIn, "admin"),
        ];
        let active = [
            (ClientRole::Connections, "conn-a", "admin"),
            (ClientRole::SignIn, "signin-b", "admin"),
        ];
        assert_eq!(
            census_slot("conn-a", &active, &configured),
            Some(("connections", "admin", false))
        );
        assert_eq!(
            census_slot("signin-b", &active, &configured),
            Some(("signin", "admin", false))
        );
        assert_eq!(
            census_slot("conn-env", &active, &configured),
            Some(("connections", "env", true)),
            "the env pair behind the record refreshes only"
        );
        assert_eq!(census_slot("rotated-away", &active, &configured), None);
        // A sign-in record whose id is also the connections env pair: the
        // trial list holds it once, under connections — but it is the
        // sign-in slot's own client, not a shadow.
        let configured = [
            client("conn-a", ClientRole::Connections, "admin"),
            client("signin-b", ClientRole::Connections, "env"),
        ];
        assert_eq!(
            census_slot("signin-b", &active, &configured),
            Some(("signin", "admin", false))
        );
    }
}
