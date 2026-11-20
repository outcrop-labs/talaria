// /api/brief. GET → the caller's daily brief: the assistant-assembled digest of what
// needs them. The read sweeps-if-due first (within a budget), then answers with
// the document — or with WHICH kind of nothing, because the three absences
// render differently and collapsing them into one empty state is how a surface
// tells a person "you are all clear" over a brief that simply has not been
// written yet.

use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_daily_brief::{BriefRead, BriefUser, get_brief, real_brief_deps, sweep_if_due};
use talaria_error::internal;
use talaria_session::require_user;
use talaria_state::AppState;

/// How long a read waits for the sweep it started before answering anyway.
///
/// The read sweeps first so a person opening the surface gets the fresh page
/// rather than the stale one and a rewrite a moment later — and a typical
/// sweep fits (most finish well under a second). The tail does not: a sweep
/// that drafts replies or asks the model for a note has taken one to two
/// minutes, past the client's 30 s deadline, so the read errored and was
/// retried into the same wait. Past this budget the read answers with the page
/// as it stands; the sweep carries on detached, still holding its claim, and
/// its append publishes the brief event that refetches the page. The job's
/// claim means a read finds a sweep due only when the job is late, so this is
/// the rare path, not the ordinary one.
const ON_READ_SWEEP_BUDGET: Duration = Duration::from_secs(2);

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
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let user = BriefUser::from(&user);
    let deps = real_brief_deps(&state).await;
    // Sweep BEFORE the read, not after, and only if the throttle allows it. A
    // person opening the surface is the one moment latency is visible, and
    // sweeping afterwards would hand them the stale page and the fresh one a
    // realtime nudge later — a document that visibly rewrites itself on
    // arrival, which is the exact impression an append-only brief must never
    // give. A failed sweep is logged and the read still answers with the page
    // as it stands.
    //
    // WITHIN A BUDGET (ON_READ_SWEEP_BUDGET). The sweep runs as its own task,
    // so one that outlives the budget is not cancelled: it finishes, holding
    // its claim, and publishes what it appends.
    let sweep = {
        let deps = deps.clone();
        let user = user.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            match sweep_if_due(&deps, &user).await {
                Ok(Some(_)) => tracing::info!(
                    "[brief] on-read sweep ran in {}ms",
                    started.elapsed().as_millis()
                ),
                Ok(None) => {}
                Err(e) => tracing::error!("[brief] on-read sweep failed: {e}"),
            }
        })
    };
    if tokio::time::timeout(ON_READ_SWEEP_BUDGET, sweep)
        .await
        .is_err()
    {
        tracing::info!(
            "[brief] on-read sweep still running after {}ms; answering with the page as it stands",
            ON_READ_SWEEP_BUDGET.as_millis()
        );
    }
    Ok(match get_brief(&deps, &user, query.tz.as_deref()).await {
        Ok(read) => brief_body(read).into_response(),
        Err(e) => internal("[brief] read failed", e),
    })
}

/// What GET /api/brief answers for a read: the document, or the absent
/// literal in wire key order (absent, nextAt, agent). Every absence carries
/// `agent` — the surface offers assistant settings from the empty state too —
/// and 'pending' is the only kind whose nextAt is knowable; the other two
/// answer null. One spelling, because POST /api/brief/item answers with
/// exactly this value after a mark and the client puts it straight into the
/// cache the GET fills.
pub(crate) fn brief_body(read: BriefRead) -> Json<Value> {
    match read {
        BriefRead::Document(doc) => Json(serde_json::to_value(doc).unwrap_or(Value::Null)),
        BriefRead::Absent(kind, next_at, agent) => {
            Json(json!({ "absent": kind, "nextAt": next_at, "agent": agent }))
        }
    }
}
