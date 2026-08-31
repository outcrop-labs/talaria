// /api/brief — port of ui/src/routes/api/brief.ts.
// GET → the caller's daily brief: the assistant-assembled digest of what
// needs them. The read sweeps-if-due first, then answers with the document —
// or with WHICH kind of nothing, because the three absences render
// differently and collapsing them into one empty state is how a surface
// tells a person "you are all clear" over a brief that simply has not been
// written yet.

use crate::daily_brief::{BriefRead, BriefUser, get_brief, real_brief_deps, sweep_if_due};
use crate::error::thrown_internal_error;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// `?tz=` — the reader's IANA zone, from their browser. The org config's zone
/// is a server-side default; without this, a person's evening can read as the
/// small hours of tomorrow in UTC and hide a brief that exists. Validated and
/// discarded inside `get_brief` if unparseable.
#[derive(serde::Deserialize)]
pub struct BriefQuery {
    tz: Option<String>,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<BriefQuery>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let user = BriefUser::from(&user);
    let deps = real_brief_deps(&state).await;
    // Sweep BEFORE the read, not after, and only if the throttle allows it. A
    // person opening the surface is the one moment latency is visible, and
    // sweeping afterwards would hand them the stale page and the fresh one a
    // realtime nudge later — a document that visibly rewrites itself on
    // arrival, which is the exact impression an append-only brief must never
    // give. Detached would be worse for the same reason; a failed sweep is
    // logged and the read still answers with the page as it stands.
    if let Err(e) = sweep_if_due(&deps, &user).await {
        tracing::error!("[brief] on-read sweep failed: {e}");
    }
    match get_brief(&deps, &user, query.tz.as_deref()).await {
        Ok(BriefRead::Document(doc)) => Json(doc).into_response(),
        // The absent literal in TS key order: absent, nextAt, agent. Every
        // absence carries `agent` — the surface offers assistant settings
        // from the empty state too — and 'pending' is the only kind whose
        // nextAt is knowable; the other two answer null.
        Ok(BriefRead::Absent(kind, next_at, agent)) => {
            Json(json!({ "absent": kind, "nextAt": next_at, "agent": agent })).into_response()
        }
        Err(e) => {
            tracing::error!("[brief] read failed: {e}");
            thrown_internal_error()
        }
    }
}
