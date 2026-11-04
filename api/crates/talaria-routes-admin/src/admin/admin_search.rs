// /api/admin/search. LIVE WEB SEARCH — where it points, and whether it is
// actually answering.
//
// WHY THE REACHABILITY CHECK IS AN ADMIN SURFACE AND NOT A LOG LINE. When
// search is down, the way an operator currently finds out is that an agent
// tells somebody it could not look something up — a model's excuse is a
// terrible monitoring channel, and it arrives one conversation at a time.
// This makes the answer a thing you can ask for, and `web_search`'s own
// error text is what it reports, so the sentence an admin reads is the
// sentence the agent got.
//
// THE ENV VAR WINS, deliberately: a self-host that already runs SearXNG
// sets SEARXNG_URL and should not have that silently overridden by a
// setting somebody typed once. The GET says which is in force so the field
// is never a lie. Set but blank is not set (`talaria_search::env_search_url`
// is the one reader of it), so the field is editable and what is saved there
// is what is in force.
//
// TWO VANTAGE POINTS, TWO ANSWERS. `reachable` is the app's own view (what
// Research dials). `agentReachable` is the same canary run from inside a
// running agent against the URL the render hands the agents
// (`talaria_agent_web_search::probe`): a SearXNG bound to the host's
// loopback answers the app and refuses every agent, and only the second
// answer shows it. `agentReachable: null` means there was nothing to probe —
// no URL handed to the agents, or no agent running — with `agentError`
// saying which. Both probes run at once.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use talaria_agent_web_search::agent_search_reachable;
use talaria_api_facades::gateway::settings::{get_setting, set_setting};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::parse;
use talaria_error::{house_error, internal, object_or_400};
use talaria_search::{SEARCH_URL_KEY, env_search_url, real_deps, search_reachable, search_url};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    Ok(Json(answer(&state).await).into_response())
}

/// Where search points and whether it answers, from both vantage points —
/// the GET's body, and the PUT's once it has saved, so the panel takes the
/// PUT's answer as the field's new state instead of probing all over again.
async fn answer(state: &AppState) -> Value {
    let stored = get_setting(&state.pg, SEARCH_URL_KEY, Value::String(String::new())).await;
    let deps = real_deps();
    let (health, agents) = tokio::join!(
        search_reachable(&state.pg, &deps),
        agent_search_reachable(&state.pg)
    );
    // The same reader the URL in force comes from: SEARXNG_URL set but
    // blank is unset there, and reads false here.
    let from_env = env_search_url().is_some();
    serde_json::json!({
        "url": search_url(&state.pg).await,
        "stored": stored,
        "fromEnv": from_env,
        "reachable": health.ok,
        "error": health.error,
        "agentReachable": agents.ok,
        "agentError": agents.error,
        "probedFrom": agents.probed_from,
        "agentUrl": agents.url,
    })
}

pub async fn put(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // url max 300 — empty is a real instruction.
    let raw = match talaria_body::string_member(obj, "url", 0, 300) {
        Ok(u) => u,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Empty means "use the environment (SEARXNG_URL), or none" — nothing is
    // bundled any more — and an admin clearing the field is a real
    // instruction, not a validation failure.
    // strip ONE trailing slash.
    let url = raw.trim().strip_suffix('/').unwrap_or(raw.trim());
    if !url.is_empty() && !(url.starts_with("http://") || url.starts_with("https://")) {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "the search URL must start with http:// or https://",
        ));
    }
    if let Err(e) = set_setting(&state.pg, SEARCH_URL_KEY, &Value::String(url.to_string())).await {
        return Ok(internal("[admin/search] setting write failed", e));
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "search.configure",
            target_type: "setting",
            target_id: Some(SEARCH_URL_KEY),
            target_label: None,
            before: None,
            after: Some(serde_json::json!({ "url": url })),
        },
    )
    .await;

    // TESTED AFTER SAVING, not before: the point of the button is to tell an
    // admin whether what they just saved works, and a check against the old
    // value would answer a question nobody asked. The body is the GET's.
    Ok(Json(answer(&state).await).into_response())
}
