// /api/admin/search/agents. AGENT WEB SEARCH — the one provider every fleet
// agent's own Hermes searches with (talaria-agent-web-search). GET is the
// setting with its keys MASKED (which vendors hold one, never the value),
// the backends this build offers, and what the render hands every agent
// from it (`rendered`: the `web:` keys, the env names, whether SEARXNG_URL
// is rendered, and the render's warnings), computed by the render itself so
// the panel never restates its rules. PUT patches it under the row lock.
//
// AGENTS ONLY. Research keeps its own search stage on the workspace search
// URL (/api/admin/search) or a registered search MCP server; this setting
// changes nothing there, and the panel says so.
//
// THE WRITE ROLLS NOTHING. The renderer reads this key on every render, so
// the choice lands on each agent at its next render or roll — the same rule
// as the org toolset floor. A vendor key reaches a container only through
// its environment, so a running agent needs the roll.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_web_search::{
    AgentWebSearch, BACKENDS, Patch, SETTING_KEY, apply, read, render, searxng_rendered, update,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{nullish_member, optional_boolean_member, optional_max_string_member, parse};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_admin, secretbox_or_500};
use talaria_state::AppState;

/// The setting as a client may see it: keys folded to set-flags, and the
/// render's own read-back with env NAMES only.
fn view(cfg: &AgentWebSearch, search_url: &str) -> Value {
    let searxng = searxng_rendered(search_url);
    let r = render(cfg, searxng);
    json!({
        "backend": cfg.backend,
        "extractBackend": cfg.extract_backend,
        "allowKeylessPublicTiers": cfg.allow_public_tiers,
        "keys": cfg.keys.keys().map(|v| (v.clone(), Value::Bool(true))).collect::<serde_json::Map<_, _>>(),
        "searchUrl": search_url,
        "backends": BACKENDS.iter().map(|b| json!({
            "name": b.name,
            "label": b.label,
            "keyEnv": b.key_env,
            "extract": b.extract,
            "publicTier": b.public_tier,
            "sdkOnFirstUse": b.sdk_on_first_use,
        })).collect::<Vec<_>>(),
        "rendered": {
            "web": r.web.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect::<serde_json::Map<_, _>>(),
            "env": r.keys.iter().map(|(env, _)| *env).collect::<Vec<_>>(),
            "searxngUrl": searxng,
            "warnings": r.warnings,
        },
    })
}

/// What the audit row records: the choice, and WHICH vendors hold a key.
fn audit_shape(cfg: &AgentWebSearch) -> Value {
    json!({
        "backend": cfg.backend,
        "extractBackend": cfg.extract_backend,
        "allowKeylessPublicTiers": cfg.allow_public_tiers,
        "keys": cfg.keys.keys().collect::<Vec<_>>(),
    })
}

pub async fn get(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let cfg = read(&state.pg).await;
    let search_url = talaria_search::search_url(&state.pg).await;
    Ok(Json(view(&cfg, &search_url)).into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;

    // backend / extractBackend: a string picks (empty hands it back to
    // Hermes), null clears, absent keeps. keys: { <vendor>: string | null }
    // — a string replaces that vendor's key, null clears it.
    let backend = match nullish_member(obj, "backend", |o, k| optional_max_string_member(o, k, 40))
    {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let extract_backend = match nullish_member(obj, "extractBackend", |o, k| {
        optional_max_string_member(o, k, 40)
    }) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let allow_public_tiers = match optional_boolean_member(obj, "allowKeylessPublicTiers") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let mut keys: Vec<(String, Option<String>)> = Vec::new();
    match obj.get("keys") {
        None => {}
        Some(Value::Object(m)) => {
            for (vendor, v) in m {
                match v {
                    Value::Null => keys.push((vendor.clone(), None)),
                    Value::String(s) => keys.push((vendor.clone(), Some(s.clone()))),
                    other => {
                        return Ok(house_error(
                            StatusCode::BAD_REQUEST,
                            &format!(
                                "keys.{vendor}: expected a string or null, received {}",
                                talaria_body::zod_type_name(other)
                            ),
                        ));
                    }
                }
            }
        }
        Some(other) => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &talaria_body::object_msg(talaria_body::zod_type_name(other)),
            ));
        }
    }
    let patch = Patch {
        backend,
        extract_backend,
        keys,
        allow_public_tiers,
    };

    let search_url = talaria_search::search_url(&state.pg).await;
    // The secretbox only when a key is being sealed: switching to SearXNG or
    // back to Hermes's default must not 500 on a box that cannot seal.
    let sb = if patch.keys.iter().any(|(_, k)| k.is_some()) {
        Some(secretbox_or_500(&state, "[admin/search/agents] secretbox").await?)
    } else {
        None
    };
    let seal = |plain: &str| -> Result<String, String> {
        sb.as_ref()
            .ok_or_else(|| "no secretbox to seal the key with".to_string())?
            .seal(plain)
            .map_err(|e| e.to_string())
    };
    // Read, apply and write under the row lock: a concurrent save or a key
    // rotation cannot be overwritten with the copy this request read.
    let (cur, next) = match update(&state.pg, |cur| {
        apply(cur, &patch, !search_url.is_empty(), seal)
    })
    .await
    {
        Ok(Ok(pair)) => pair,
        Ok(Err(msg)) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        Err(e) => return Ok(internal("[admin/search/agents] setting write failed", e)),
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "search.agents_configure",
            target_type: "setting",
            target_id: Some(SETTING_KEY),
            target_label: None,
            before: Some(audit_shape(&cur)),
            after: Some(audit_shape(&next)),
        },
    )
    .await;
    Ok(Json(view(&next, &search_url)).into_response())
}
