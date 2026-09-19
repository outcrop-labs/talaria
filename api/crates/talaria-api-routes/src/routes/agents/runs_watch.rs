// GET /api/runs/{id}/watch → SSE of the run's WORK TERMINAL: the agent's
// own stream events (words as they land, tool calls as they start) as one
// JSON line per `data:` frame — first a bounded replay of the current
// turn's tail, then live frames as they publish. Same ACL as the run's
// event stream: the audience that may see the run may see the work.
//
// THE PAYLOADS ARE WATCH LINES (see `watch_line` in the transport): single
// objects `{t: "d"|"r"|"tool"|"err", v: …}` — never raw SSE from the agent
// gateway, never tool RESULTS (the persona stream reports names only), so
// the terminal shows the shape of the work without becoming a second
// disclosure surface for tool output.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use futures_util::StreamExt;
use std::convert::Infallible;
use talaria_error::{house_error, thrown_internal_error};
use talaria_realtime_watch::{RealtimeDeps, RunWatchVerdict, may_watch_run, real_watch_deps};
use talaria_session::require_user;
use talaria_state::AppState;

/// How much of the current turn's tail a late watcher replays: enough to
/// read where the agent is, small enough that the modal opens instantly.
const REPLAY_BYTES: usize = 16 * 1024;

pub async fn get(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let verdict = match may_watch_run(&user.id, &run_id, &real_watch_deps(state.pg.clone())).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[runs/watch] watch gate failed for {run_id}: {e}");
            return thrown_internal_error();
        }
    };
    if verdict != RunWatchVerdict::Ok {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }

    let channel = format!("run-watch:{run_id}");
    let tail_key = format!("{channel}:tail");

    // The replay first: the tail's LAST REPLAY_BYTES, cut to a line boundary
    // so the first live frame the client parses is whole.
    let replay: String = match state.redis().await {
        Ok(mut conn) => {
            let len: isize = redis::cmd("STRLEN")
                .arg(&tail_key)
                .query_async(&mut conn)
                .await
                .unwrap_or(0);
            let start = (len - REPLAY_BYTES as isize).max(0);
            let raw: String = redis::cmd("GETRANGE")
                .arg(&tail_key)
                .arg(start)
                .arg(-1)
                .query_async(&mut conn)
                .await
                .unwrap_or_default();
            match raw.find('\n') {
                // Drop the possibly-cut first line.
                Some(i) if start > 0 => raw[i + 1..].to_string(),
                _ => raw,
            }
        }
        Err(_) => String::new(),
    };

    let rt = RealtimeDeps::publish_only(state.redis().await.ok());
    let rx = (rt.subscribe)(channel).await;

    let replay_lines: Vec<String> = replay
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| format!("data: {l}\n\n"))
        .collect();
    let body = axum::body::Body::from_stream(
        futures_util::stream::iter(replay_lines.into_iter().map(Ok::<_, Infallible>)).chain(
            futures_util::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|line| {
                    let frame = format!("data: {line}\n\n");
                    (Ok::<_, Infallible>(frame), rx)
                })
            }),
        ),
    );
    let mut res = Response::new(body);
    res.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/event-stream"),
    );
    res.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache, no-transform"),
    );
    res
}
