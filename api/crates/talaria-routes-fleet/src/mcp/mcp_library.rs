// /api/mcp/library.
// GET → the MCP server catalog, served from the LOCAL MIRROR of the official
// registry (`talaria_mcp_library::catalog`): `?featured=1` is the editorial
// shelf, `?q=` searches title/domain/name/description with full text, and
// `tier=official|verified|community` (best tier to include) / `kind=hosted|
// package` / `limit=` narrow it. Rows the liveness job found unreachable are
// left out and counted (`hiddenDead`); `includeDead=1` puts them back, last,
// each carrying `live: {state: "dead", checkedAt, detail}`. Every server has
// `live` (null = never checked). Both arms answer `{servers, hiddenDead,
// catalog}` — `catalog` is the mirror's status (`count`, `syncedAt`,
// `syncing`, `phase: empty|syncing|ready`, `error`, `liveness: {checked, dead,
// unknown, pending}`) so the picker can say "syncing" or "no match" and mean
// it. Backs the Add-server picker and the agent MCP tab.
// POST /api/mcp/library/sync → start a sync now (the first-boot nudge and the
// picker's "Sync now"); answers `{ok, catalog}` immediately — the walk runs
// behind it.
// POST /api/mcp/library/liveness → admin: run one liveness batch now (300
// hosted rows, never-checked first); answers `{ok, running, catalog}`
// immediately — the probes run behind it and each verdict lands as it comes.
// GET /api/mcp/library/featured → admin: the operator's featured edits
// `{domains, hidden, standIns, builtins, shelf}`. PUT the same shape (the
// first three) to replace them: domains are bare hosts, stand-in urls https.
// The shelf answers the new list at once; stand-in rows are written before
// the PUT returns and the publisher pass re-runs behind it. Audited as
// `mcp.featured`.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_api_facades::mcp::library::catalog::{self, SearchFilters, SearchPage};
use talaria_api_facades::mcp::library::featured::{self, FeaturedConfig};
use talaria_api_facades::mcp::library::liveness;
use talaria_api_facades::mcp::library::{Tier, library};
use talaria_audit::{AuditEntry, log_audit};
use talaria_error::{house_error, internal};
use talaria_session::{actor_of, require_admin, require_perm};
use talaria_state::AppState;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryQuery {
    q: Option<String>,
    featured: Option<String>,
    tier: Option<String>,
    kind: Option<String>,
    limit: Option<i64>,
    include_dead: Option<String>,
}

fn page_wire(page: &SearchPage) -> Value {
    json!({
        "servers": page.hits.iter().map(catalog::hit_wire).collect::<Vec<_>>(),
        "hiddenDead": page.hidden_dead,
    })
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<LibraryQuery>,
) -> Result<Response, Response> {
    require_perm(&state, &headers, "agents.manage").await?;
    let lib = library();
    let status = catalog::status(&state.pg).await;
    // An empty mirror on a running instance means the first sync has not
    // happened yet (fresh install, or the job never got its lease) — nudge it
    // from here rather than wait for the next tick. Rate-limited by the
    // catalog's own attempt stamp, so an unreachable registry is one try
    // per window rather than one per page load.
    if status.nudgeable {
        catalog::trigger_sync(state.pg.clone(), lib.clone());
    }
    let include_dead = matches!(query.include_dead.as_deref(), Some("1" | "true"));
    let page = if query.featured.as_deref() == Some("1") {
        catalog::featured(&state.pg, &lib, include_dead).await
    } else {
        let filters = SearchFilters {
            max_tier: query.tier.as_deref().and_then(Tier::parse),
            kind: query
                .kind
                .as_deref()
                .filter(|k| matches!(*k, "hosted" | "package"))
                .map(str::to_string),
            limit: query.limit,
            include_dead,
        };
        // absent and empty are the same query.
        catalog::search(&state.pg, &lib, query.q.as_deref().unwrap_or(""), &filters).await
    };
    let mut body = page_wire(&page);
    body["catalog"] = catalog::status_wire(&status);
    Ok(Json(body).into_response())
}

pub async fn sync(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_perm(&state, &headers, "agents.manage").await?;
    catalog::trigger_sync(state.pg.clone(), library());
    let status = catalog::status(&state.pg).await;
    Ok(Json(json!({ "ok": true, "catalog": catalog::status_wire(&status) })).into_response())
}

pub async fn liveness(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    liveness::trigger_liveness(state.pg.clone());
    let status = catalog::status(&state.pg).await;
    Ok(Json(json!({
        "ok": true,
        "running": true,
        "catalog": catalog::status_wire(&status),
    }))
    .into_response())
}

pub async fn featured_get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let cfg = featured::load(&state.pg).await;
    Ok(Json(featured::wire(&cfg)).into_response())
}

pub async fn featured_put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let input: FeaturedConfig = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &format!(
                    "expected {{domains: string[], hidden: string[], standIns: [{{domain, title, url, description?}}]}} — {e}"
                ),
            ));
        }
    };
    let cfg = match featured::validate(input) {
        Ok(c) => c,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let before = featured::load(&state.pg).await;
    if let Err(e) = featured::save(&state.pg, &cfg).await {
        return Ok(internal("[mcp/featured] save failed", e));
    }
    // The rows first, so the liveness batch the operator may kick next
    // already has them; the well-known/curated resolution rides the sync.
    if let Err(e) = catalog::write_operator_stand_ins(&state.pg, &cfg).await {
        return Ok(internal("[mcp/featured] stand-in rows failed", e));
    }
    catalog::trigger_sync(state.pg.clone(), library());
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "mcp.featured",
            target_type: "mcp-library",
            target_id: None,
            target_label: Some("featured shelf"),
            before: serde_json::to_value(&before).ok(),
            after: serde_json::to_value(&cfg).ok(),
        },
    )
    .await;
    Ok(Json(featured::wire(&cfg)).into_response())
}
