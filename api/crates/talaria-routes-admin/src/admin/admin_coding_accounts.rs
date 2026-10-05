// /api/admin/coding-accounts. The org's policy for coding accounts: whether
// the feature exists at all, and which services it may reach.
//
// TWO CONTROLS, ON PURPOSE. Turning the feature on does not permit anything —
// an admin then says which services the org allows, because "developers may
// sign agents in to coding subscriptions" and "developers may sign agents in
// to *anything omp supports*" are different decisions. An org that wants
// Copilot and Codex but not a personal Claude Max subscription can say exactly
// that.
//
// REVOKING A SERVICE DOES NOT DELETE ITS CREDENTIALS. An account for a service
// that leaves the allowlist stops being served to harnesses immediately — the
// snapshot omits it and resolution falls back to the gateway — but the row
// stays, so re-permitting the service brings the account back without a
// re-login. Throwing away a credential should take someone saying "sign this
// out", not an allowlist edit.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{boolean_member, parse, string_array_member};
use talaria_coding_accounts as ca;
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;

// doc: The org's coding-account policy, with omp's full sign-in roster so an
// doc: admin can see every service the harness could reach — not only the ones
// doc: already allowed. A roster read failure answers an empty roster rather
// doc: than a 500: the toggle and the current allowlist still render.
pub async fn get(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let enabled = ca::enabled(&state.pg).await;
    let permitted = ca::permitted_services(&state.pg).await;
    let (roster, roster_error) = match talaria_omp_auth::providers().await {
        Ok(r) => (r, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    // One row per SERVICE, not per sign-in door: `openai-codex` and
    // `openai-codex-device` are one subscription reached two ways, and an
    // allowlist with both would ask the admin a question that has one answer.
    let mut services: Vec<Value> = Vec::new();
    for entry in &roster {
        let store_as = entry
            .get("storeAs")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if store_as.is_empty() || services.iter().any(|s| s["id"] == json!(store_as)) {
            continue;
        }
        services.push(json!({
            "id": store_as,
            "name": entry.get("name").cloned().unwrap_or(Value::Null),
            "flow": entry.get("flow").cloned().unwrap_or(Value::Null),
            "permitted": permitted.contains(&store_as),
        }));
    }
    // A service an admin permitted that the installed omp no longer offers
    // still shows, flagged, so a silently-dropped upstream provider is
    // visible instead of vanishing from the panel with accounts still on it.
    for id in &permitted {
        if !services.iter().any(|s| s["id"] == json!(id)) {
            services.push(json!({
                "id": id, "name": id, "flow": Value::Null,
                "permitted": true, "unavailable": true,
            }));
        }
    }
    Ok(Json(json!({
        "enabled": enabled,
        "permitted": permitted,
        "services": services,
        "rosterError": roster_error,
    }))
    .into_response())
}

// doc: Set the feature toggle and the permitted-service allowlist. Services
// doc: not on omp's roster are refused by name rather than stored as typos.
pub async fn put(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let enabled = match boolean_member(obj, "enabled") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // An empty array clears the allowlist, which is a real choice: the feature
    // on and nothing permitted yet is the state a fresh install should be in.
    let services = match string_array_member(obj, "services", 0, 100, 0, 100) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    // Validate against omp's roster when we can reach it. When we cannot, the
    // toggle still applies but the allowlist is left alone — storing an
    // unvalidated list would be the one way to end up permitting a service
    // that does not exist.
    match talaria_omp_auth::providers().await {
        Ok(roster) => {
            let known: Vec<String> = roster
                .iter()
                .filter_map(|e| e.get("storeAs").and_then(Value::as_str).map(str::to_string))
                .collect();
            let mut wanted: Vec<String> = Vec::new();
            for s in services {
                let s = s.trim().to_string();
                if s.is_empty() {
                    continue;
                }
                if !known.contains(&s) {
                    return Ok(house_error(
                        StatusCode::BAD_REQUEST,
                        &format!("\"{s}\" is not a service omp can sign in to"),
                    ));
                }
                if !wanted.contains(&s) {
                    wanted.push(s);
                }
            }
            ca::set_permitted_services(&state.pg, &wanted)
                .await
                .map_err(|e| internal("[admin/coding] allowlist write failed", e))?;
        }
        Err(e) => {
            tracing::warn!("[admin/coding] roster unreachable, allowlist left as is: {e}");
        }
    }
    ca::set_enabled(&state.pg, enabled)
        .await
        .map_err(|e| internal("[admin/coding] toggle write failed", e))?;

    let actor = actor_of(&user);
    let permitted = ca::permitted_services(&state.pg).await;
    let after = json!({ "enabled": enabled, "services": permitted });
    let pg = state.pg.clone();
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "admin.coding_accounts",
                target_type: "settings",
                target_id: Some("coding_accounts"),
                target_label: None,
                before: None,
                after: Some(after),
            },
        )
        .await;
    });
    get(State(state), headers).await
}
