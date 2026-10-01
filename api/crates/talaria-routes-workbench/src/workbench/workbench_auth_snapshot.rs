// /api/workbench/auth/v1/snapshot. The calling agent's coding accounts, as
// omp's auth-broker protocol spells them.
//
// Conditional-GET shaped: the client sends `If-None-Match: "<generation>"` and
// we answer 304 while nothing has changed. `?wait=<ms>` makes that a long
// poll, which is what keeps an idle agent from spinning on this route — we
// honour it by waiting for the generation to move rather than answering 304
// immediately and inviting a tight loop.
//
// The access token goes out; the refresh token never does (see coding_broker).

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;

use talaria_coding_accounts as ca;
use talaria_error::internal;
use talaria_state::AppState;

use super::coding_broker::{broker_agent, generation, refresher_block, snapshot_entry};

/// A long poll is bounded well under any sane proxy's idle timeout, and the
/// client re-polls on its own.
const MAX_WAIT: Duration = Duration::from_secs(25);
const POLL_STEP: Duration = Duration::from_millis(500);

/// `"N"`, `W/"N"` or bare `N` — the client sends the quoted form.
fn parse_generation_tag(header: Option<&HeaderValue>) -> Option<i64> {
    let raw = header?.to_str().ok()?.trim();
    let raw = raw.strip_prefix("W/").unwrap_or(raw).trim();
    let raw = raw.strip_prefix('"').unwrap_or(raw);
    let raw = raw.strip_suffix('"').unwrap_or(raw);
    raw.parse::<i64>().ok().filter(|g| *g >= 0)
}

// doc: The calling agent's coding-account credentials for its omp harness:
// doc: access tokens with `__remote__` in the refresh slot, so this instance
// doc: stays the only thing that can refresh them. Scoped by the agent's own
// doc: key — an agent can read nothing but its own accounts.
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, Response> {
    let agent_id = broker_agent(&state, &headers).await?;
    let known = parse_generation_tag(headers.get(axum::http::header::IF_NONE_MATCH));
    let wait = q
        .get("wait")
        .and_then(|w| w.parse::<u64>().ok())
        .map(|ms| Duration::from_millis(ms).min(MAX_WAIT));

    let mut current = generation(&state, &agent_id).await;
    // Long poll: hold the request open until the agent's credentials actually
    // change, so an idle harness costs one parked connection rather than a
    // request every tick.
    if let (Some(known), Some(wait)) = (known, wait)
        && current == known
    {
        let deadline = tokio::time::Instant::now() + wait;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(POLL_STEP).await;
            current = generation(&state, &agent_id).await;
            if current != known {
                break;
            }
        }
    }
    if known == Some(current) {
        return Ok((
            StatusCode::NOT_MODIFIED,
            [(axum::http::header::ETAG, format!("\"{current}\""))],
        )
            .into_response());
    }

    let sb = state
        .secretbox()
        .await
        .map_err(|e| internal("[workbench/auth] secretbox failed", e))?;
    let accounts = ca::open_accounts(&state.pg, &sb, &agent_id)
        .await
        .map_err(|e| internal("[workbench/auth] credential read failed", e))?;
    let mut credentials: Vec<Value> = Vec::new();
    for account in &accounts {
        // A disabled account stays in the table (with its reason) but is not
        // served: the harness must stop using it, and only a re-login in the
        // UI brings it back.
        if account.disabled {
            continue;
        }
        if let Some(entry) = snapshot_entry(&state, account).await {
            credentials.push(entry);
        }
    }

    let now = talaria_agent_auth::now_ms();
    let body = json!({
        "generation": current,
        "generatedAt": now,
        "serverNowMs": now,
        "refresher": refresher_block(),
        "credentials": credentials,
    });
    Ok((
        StatusCode::OK,
        [(axum::http::header::ETAG, format!("\"{current}\""))],
        axum::Json(body),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_etag_spelling_the_client_may_send_parses() {
        let h = |v: &str| HeaderValue::from_str(v).unwrap();
        assert_eq!(parse_generation_tag(Some(&h("\"7\""))), Some(7));
        assert_eq!(parse_generation_tag(Some(&h("W/\"7\""))), Some(7));
        assert_eq!(parse_generation_tag(Some(&h("7"))), Some(7));
        assert_eq!(parse_generation_tag(Some(&h(" \"7\" "))), Some(7));
    }

    #[test]
    fn nonsense_etags_are_not_a_generation() {
        let h = |v: &str| HeaderValue::from_str(v).unwrap();
        assert_eq!(parse_generation_tag(Some(&h("\"-1\""))), None);
        assert_eq!(parse_generation_tag(Some(&h("\"abc\""))), None);
        assert_eq!(parse_generation_tag(None), None);
    }
}
