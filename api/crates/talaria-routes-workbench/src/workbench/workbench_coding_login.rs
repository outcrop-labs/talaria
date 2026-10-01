// /api/workbench/coding/login. Signing an agent in to a coding account.
//
// THE SHAPE OF A SIGN-IN. The omp auth bridge runs the provider's own flow and
// reports what it is waiting for; this route relays that to the UI and relays
// the person's answers back. Three things can come out of a poll:
//   - a URL (and sometimes a device code) to open;
//   - a question the provider asked (an enterprise domain, a region, a login
//     method), or a request to paste the authorization code;
//   - a finished credential.
// Every provider's flow is one of those three in some order, so the UI needs
// one state machine rather than twenty-three.
//
// WHY THE PASTE EXISTS. Most authorization-code providers redirect to
// `http://localhost:<port>` — a port on whatever host is running the flow,
// which is this instance. When Talaria runs on the person's own machine (a dev
// stack) their browser reaches it and the login completes by itself. When it
// does not (every deployed instance), the redirect lands nowhere and the
// person pastes the code or the failed redirect URL instead. That fallback is
// the provider's own, declared in omp's auth rules, not something invented
// here.
//
// THE CREDENTIAL IS READ EXACTLY ONCE. The bridge serves a finished login's
// credential on a single poll and forgets it, so this route seals it into
// Postgres in the same request that reads it. There is no second chance and no
// window where plaintext sits in the bridge waiting to be collected.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_audit::spawn_audit;
use talaria_body::{parse, string_member, uuid_member};
use talaria_coding_accounts as ca;
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::actor_of;
use talaria_state::AppState;

use super::coding_gate::{agent_exists, require_agent_editor};

/// A login in flight, keyed so a poll can find its way back to the agent it
/// was started for. The bridge knows nothing about agents — it performs flows
/// — so the binding lives here, in memory, for the life of the flow.
///
/// In memory rather than in Postgres on purpose: a login that does not finish
/// before this process restarts is a login the person must start again
/// anyway, and the alternative is a table of half-finished OAuth attempts.
struct Pending {
    agent_id: String,
    login_provider: String,
    store_as: String,
    user_id: String,
    started_ms: i64,
}

fn pending() -> &'static std::sync::Mutex<std::collections::HashMap<String, Pending>> {
    static P: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Pending>>> =
        std::sync::OnceLock::new();
    P.get_or_init(Default::default)
}

/// Matches the bridge's own session lifetime, so a row cannot outlive the
/// flow it describes.
const PENDING_TTL_MS: i64 = 15 * 60_000;

fn sweep_pending() {
    let now = talaria_agent_auth::now_ms();
    if let Ok(mut map) = pending().lock() {
        map.retain(|_, p| now - p.started_ms < PENDING_TTL_MS);
    }
}

// doc: Start a coding-account sign-in for one agent. Answers the login session
// doc: to poll: a URL to open, a question to answer, or both.
pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let agent_id = match uuid_member(obj, "agentId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let user = require_agent_editor(&state, &headers, &agent_id).await?;
    if !agent_exists(&state, &agent_id).await {
        return Ok(house_error(StatusCode::NOT_FOUND, "unknown agent"));
    }
    let provider = match string_member(obj, "provider", 1, 100) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    // The roster tells us the STORE id, which is the service the allowlist is
    // about: `openai-codex-device` and `openai-codex` are one service reached
    // through two doors, and permitting one permits both.
    let roster = talaria_omp_auth::providers()
        .await
        .map_err(|e| house_error(StatusCode::BAD_GATEWAY, &e))?;
    let Some(entry) = roster.iter().find(|e| e["id"] == json!(provider)) else {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "that service is not one omp can sign in to",
        ));
    };
    let store_as = entry["storeAs"].as_str().unwrap_or(&provider).to_string();
    if !ca::service_permitted(&state.pg, &store_as).await {
        return Ok(house_error(
            StatusCode::FORBIDDEN,
            "this organization does not permit that service",
        ));
    }

    let session = talaria_omp_auth::login_start(&provider)
        .await
        .map_err(|e| house_error(StatusCode::BAD_GATEWAY, &e))?;
    let Some(id) = session["id"].as_str() else {
        return Err(house_error(
            StatusCode::BAD_GATEWAY,
            "the omp auth bridge returned no login session",
        ));
    };
    sweep_pending();
    if let Ok(mut map) = pending().lock() {
        map.insert(
            id.to_string(),
            Pending {
                agent_id: agent_id.clone(),
                login_provider: provider.clone(),
                store_as,
                user_id: user.id.clone(),
                started_ms: talaria_agent_auth::now_ms(),
            },
        );
    }
    spawn_audit(
        &state.pg,
        actor_of(&user),
        "agent.coding_login_start",
        "agent",
        Some(agent_id.clone()),
        Some(json!({ "provider": provider })),
    );
    Ok(axum::Json(wire(&session)).into_response())
}

// doc: Poll a sign-in. A finished flow is sealed into this instance in the
// doc: same request that reads it — the bridge serves a credential once.
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let (agent_id, login_provider, store_as, user_id) = {
        let Ok(map) = pending().lock() else {
            return Err(internal_str("login registry unavailable"));
        };
        let Some(p) = map.get(&id) else {
            return Ok(house_error(StatusCode::NOT_FOUND, "unknown login session"));
        };
        (
            p.agent_id.clone(),
            p.login_provider.clone(),
            p.store_as.clone(),
            p.user_id.clone(),
        )
    };
    let user = require_agent_editor(&state, &headers, &agent_id).await?;
    // The person who started a flow is the only one who may collect it: a
    // second editor polling someone else's in-flight login would land a
    // credential they never authorized.
    if user.id != user_id {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }

    let session = talaria_omp_auth::login_poll(&id)
        .await
        .map_err(|e| house_error(StatusCode::BAD_GATEWAY, &e))?;

    if session["phase"] == json!("done") {
        if let Ok(mut map) = pending().lock() {
            map.remove(&id);
        }
        let credential = session["credential"].clone();
        if !credential.is_object() {
            return Err(house_error(
                StatusCode::BAD_GATEWAY,
                "the sign-in finished without a credential",
            ));
        }
        let identity = ca::Identity::from_credential(&credential, session["identityKey"].as_str());
        let sb = state
            .secretbox()
            .await
            .map_err(|e| internal("[coding] secretbox failed", e))?;
        let account_id = ca::upsert_account(
            &state.pg,
            &sb,
            &agent_id,
            &store_as,
            &login_provider,
            &credential,
            &identity,
            Some(&user.id),
        )
        .await
        .map_err(|e| house_error(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
        spawn_audit(
            &state.pg,
            actor_of(&user),
            "agent.coding_login",
            "agent",
            Some(agent_id.clone()),
            Some(json!({
                "provider": store_as,
                "loginProvider": login_provider,
                "accountId": account_id,
                "email": identity.email,
                "org": identity.org_name,
            })),
        );
        return Ok(axum::Json(json!({
            "phase": "done",
            "accountId": account_id,
            "provider": store_as,
            "email": identity.email,
            "orgName": identity.org_name,
        }))
        .into_response());
    }

    if session["phase"] == json!("error")
        && let Ok(mut map) = pending().lock()
    {
        map.remove(&id);
    }
    Ok(axum::Json(wire(&session)).into_response())
}

// doc: Answer the provider's pending question, or paste the authorization code
// doc: when the browser could not reach this instance's loopback.
pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, Response> {
    let agent_id = {
        let Ok(map) = pending().lock() else {
            return Err(internal_str("login registry unavailable"));
        };
        match map.get(&id) {
            Some(p) => p.agent_id.clone(),
            None => return Ok(house_error(StatusCode::NOT_FOUND, "unknown login session")),
        }
    };
    require_agent_editor(&state, &headers, &agent_id).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // Untrimmed and generous: this carries a pasted redirect URL as readily as
    // a six-character code, and a provider's own answer format is not ours to
    // normalize.
    let value = match string_member(obj, "value", 1, 4096) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let session = talaria_omp_auth::login_input(&id, &value)
        .await
        .map_err(|e| house_error(StatusCode::BAD_GATEWAY, &e))?;
    Ok(axum::Json(wire(&session)).into_response())
}

// doc: Abandon a sign-in in flight.
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let agent_id = {
        let Ok(map) = pending().lock() else {
            return Err(internal_str("login registry unavailable"));
        };
        match map.get(&id) {
            Some(p) => p.agent_id.clone(),
            None => return Ok(axum::Json(json!({ "ok": true })).into_response()),
        }
    };
    require_agent_editor(&state, &headers, &agent_id).await?;
    let _ = talaria_omp_auth::login_cancel(&id).await;
    if let Ok(mut map) = pending().lock() {
        map.remove(&id);
    }
    Ok(axum::Json(json!({ "ok": true })).into_response())
}

/// The bridge's session as the UI reads it. The credential is never in this
/// shape — a finished login answers from the branch above, which has already
/// sealed it.
fn wire(session: &Value) -> Value {
    json!({
        "id": session["id"],
        "provider": session["provider"],
        "phase": session["phase"],
        "url": session["url"],
        "launchUrl": session["launchUrl"],
        "instructions": session["instructions"],
        "progress": session["progress"],
        "prompt": session["prompt"],
        "error": session["error"],
    })
}

fn internal_str(what: &str) -> Response {
    house_error(StatusCode::INTERNAL_SERVER_ERROR, what)
}
