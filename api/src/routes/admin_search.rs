// /api/admin/search — port of ui/src/routes/api/admin.search.ts. LIVE WEB
// SEARCH — where it points, and whether it is actually answering.
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
// is never a lie.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse};
use crate::error::{house_error, thrown_internal_error};
use crate::gateway::settings::{get_setting, set_setting};
use crate::search::{SEARCH_URL_KEY, real_deps, search_reachable, search_url};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let stored = get_setting(&state.pg, SEARCH_URL_KEY, Value::String(String::new())).await;
    let health = search_reachable(&state.pg, &real_deps()).await;
    // Boolean(process.env.SEARXNG_URL) — SET-BUT-EMPTY is false.
    let from_env = std::env::var("SEARXNG_URL")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    Json(serde_json::json!({
        "url": search_url(&state.pg).await,
        "stored": stored,
        "fromEnv": from_env,
        "reachable": health.ok,
        "error": health.error,
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
    // z.object({ url: z.string().max(300) }) — empty is a real instruction.
    let raw = match crate::body::string_member(obj, "url", 0, 300) {
        Ok(u) => u,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Empty means "use the environment / the bundled instance" — an admin
    // clearing the field is a real instruction, not a validation failure.
    // .replace(/\/$/, '') — ONE trailing slash.
    let url = raw.trim().strip_suffix('/').unwrap_or(raw.trim());
    if !url.is_empty() && !(url.starts_with("http://") || url.starts_with("https://")) {
        return house_error(
            StatusCode::BAD_REQUEST,
            "the search URL must start with http:// or https://",
        );
    }
    if let Err(e) = set_setting(&state.pg, SEARCH_URL_KEY, &Value::String(url.to_string())).await {
        tracing::error!("[admin/search] setting write failed: {e}");
        return thrown_internal_error();
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
    // value would answer a question nobody asked.
    let health = search_reachable(&state.pg, &real_deps()).await;
    Json(serde_json::json!({
        "url": search_url(&state.pg).await,
        "reachable": health.ok,
        "error": health.error,
    }))
    .into_response()
}
