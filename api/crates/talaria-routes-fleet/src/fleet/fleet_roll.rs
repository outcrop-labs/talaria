// /api/fleet/roll — roll every agent container at once, in-platform. Admin.
//   POST  queue a blue/green roll of every managed, enabled agent that is
//         running (`{"all": true}`: every managed enabled agent). Answers at
//         once; the rolls run detached, TALARIA_FLEET_ROLL_CONCURRENCY
//         (default 2) departments at a time, departments already mid-roll
//         skipped and named.
//   GET   the live picture — one row per managed agent: rolling / idle /
//         failed / done, since when, and the failure sentence. This
//         instance's registry merged with the Redis hash every instance's
//         rolls write, so a second api answers the same.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use redis::AsyncCommands;
use serde_json::{Value, json};
use talaria_api_facades::fleet::reconcile::{
    ROLL_OUTCOME_TTL_MS, ROLL_REGISTRY_KEY, ROLL_SINK, RollFailure, RollRecord, RollState,
    merge_roll_records, roll_records, start_roll_batch,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{optional_boolean_member, parse};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_admin, secretbox_or_500};
use talaria_state::AppState;

/// Point the reconcile crate's registry sink at this instance's Redis. The
/// engine crate owns no connection, so the mirror is wired from the routes
/// that roll (here and the single-agent control) — once per process, before
/// the first roll any of them starts. Best-effort: a Redis that is down
/// leaves the in-process registry complete for this instance and says so in
/// the log.
pub fn wire_roll_sink(state: &AppState) {
    let state = state.clone();
    let _ = ROLL_SINK.set(std::sync::Arc::new(move |record: RollRecord| {
        let state = state.clone();
        Box::pin(async move {
            let Ok(mut redis) = state.redis().await else {
                tracing::warn!(
                    "[fleet] roll registry: redis unavailable — this instance's picture only"
                );
                return;
            };
            let Ok(body) = serde_json::to_string(&record) else {
                return;
            };
            let written: redis::RedisResult<()> = redis
                .hset(ROLL_REGISTRY_KEY, &record.department, body)
                .await;
            if let Err(e) = written {
                tracing::warn!("[fleet] roll registry: redis write failed: {e}");
                return;
            }
            // Outcomes are kept an hour past the last write; a roll never
            // lasts that long, so the key only expires once the fleet is
            // quiet.
            let _: redis::RedisResult<()> = redis
                .expire(ROLL_REGISTRY_KEY, ROLL_OUTCOME_TTL_MS / 1000)
                .await;
        })
    }));
}

/// The other instances' records, from the Redis hash. Unreachable Redis is
/// not a failure of the read — the local registry still answers — but the
/// caller is told, so a cross-instance blind spot never reads as "idle".
async fn remote_records(state: &AppState) -> (Vec<RollRecord>, bool) {
    let Ok(mut redis) = state.redis().await else {
        return (Vec::new(), false);
    };
    let read: redis::RedisResult<std::collections::HashMap<String, String>> =
        redis.hgetall(ROLL_REGISTRY_KEY).await;
    match read {
        Ok(fields) => (
            fields
                .into_values()
                .filter_map(|v| serde_json::from_str::<RollRecord>(&v).ok())
                .collect(),
            true,
        ),
        Err(e) => {
            tracing::warn!("[fleet] roll registry: redis read failed: {e}");
            (Vec::new(), false)
        }
    }
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    wire_roll_sink(&state);
    let defs: Vec<(String, String, String)> = match sqlx::query_as(
        "select slug, department, display_name from agent_defs \
         where managed and enabled order by slug",
    )
    .fetch_all(&state.pg)
    .await
    {
        Ok(rows) => rows,
        Err(e) => return Ok(internal("[fleet] roll picture failed", e)),
    };
    let (remote, redis_ok) = remote_records(&state).await;
    let merged = merge_roll_records(roll_records(), remote);
    let by_dept: std::collections::HashMap<&str, &RollRecord> =
        merged.iter().map(|r| (r.department.as_str(), r)).collect();
    let mut rolls: Vec<Value> = Vec::new();
    let mut rolling = false;
    let mut latest: Option<&RollRecord> = None;
    for (slug, department, display_name) in &defs {
        let record = by_dept.get(department.as_str()).copied();
        if record.is_some_and(|r| r.state == RollState::Rolling) {
            rolling = true;
        }
        if let Some(r) = record
            && latest.is_none_or(|l| r.since > l.since)
        {
            latest = Some(r);
        }
        rolls.push(json!({
            "slug": slug,
            "department": department,
            "displayName": display_name,
            "state": record.map(|r| r.state).unwrap_or(RollState::Idle),
            "since": record.map(|r| r.since),
            "detail": record.and_then(|r| r.detail.clone()),
            "batch": record.and_then(|r| r.batch.clone()),
        }));
    }
    Ok(Json(json!({
        "ok": true,
        "rolling": rolling,
        "batch": latest.and_then(|r| r.batch.clone()),
        "rolls": rolls,
        "crossInstance": redis_ok,
    }))
    .into_response())
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    // An empty body is the common case ("roll what is running") and reads
    // as the empty object; `all` is the one switch.
    let parsed = if body.is_empty() {
        json!({})
    } else {
        parse(&body)
    };
    let obj = object_or_400(&parsed)?;
    // Every managed enabled agent, the stopped ones brought up too.
    let all = match optional_boolean_member(obj, "all") {
        Ok(v) => v.unwrap_or(false),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let sb = secretbox_or_500(&state, "[fleet] secretbox failed").await?;
    wire_roll_sink(&state);
    let batch = uuid::Uuid::new_v4().to_string();
    let actor = actor_of(&user);

    // Every failed roll in the batch audits the same way the single-agent
    // control does, so an operator reading the trail sees one vocabulary.
    let failure_pg = state.pg.clone();
    let failure_actor = actor.clone();
    let on_failed = std::sync::Arc::new(move |f: RollFailure| {
        let pg = failure_pg.clone();
        let actor = failure_actor.clone();
        Box::pin(async move {
            log_audit(
                &pg,
                AuditEntry {
                    actor: &actor,
                    action: "agent.roll_failed",
                    target_type: "agent",
                    target_id: Some(&f.agent_id),
                    target_label: Some(&f.display_name),
                    before: None,
                    after: Some(json!({ "error": f.error, "batch": f.batch })),
                },
            )
            .await;
        }) as futures_util::future::BoxFuture<'static, ()>
    });

    let plan = match start_roll_batch(&state.pg, &sb, &batch, all, on_failed).await {
        Ok(plan) => plan,
        Err(e) => return Ok(internal("[fleet] roll all failed to start", e)),
    };
    {
        let pg = state.pg.clone();
        let after = json!({
            "batch": plan.batch,
            "all": all,
            "rolling": plan.rolling,
            "skipped": plan.skipped,
        });
        tokio::spawn(async move {
            log_audit(
                &pg,
                AuditEntry {
                    actor: &actor,
                    action: "fleet.roll_all",
                    target_type: "fleet",
                    target_id: Some("fleet"),
                    target_label: None,
                    before: None,
                    after: Some(after),
                },
            )
            .await;
        });
    }
    Ok(Json(json!({
        "ok": true,
        "rolling": plan.rolling,
        "skipped": plan.skipped,
        "batch": plan.batch,
    }))
    .into_response())
}
