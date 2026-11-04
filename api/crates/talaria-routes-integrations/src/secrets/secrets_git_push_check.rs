// /api/secrets/git-push-check.
//
// THE PRE-PUSH GATE — the platform's own answer to "whose call is main".
// A git host's own branch protection enforces the posture where it can;
// this route is what the rendered pre-push hook asks, and it works for every
// granted repo on every registered host, including the private ones a free
// plan cannot protect. The hook fires inside the container BEFORE any bytes
// leave for the remote, with the refs git is about to push; this route
// answers allow/deny against the agent's own repo rules, in the same shape
// as the credential route beside it: agent-authenticated, never
// session-authenticated, the decision logged for the operator.
//
// A DENY NEVER LEAVES THE CONTAINER AS DATA: the hook turns it into git's
// own "pre-push declined" with the sentence on stderr, so the agent reads
// the rule where it reads every other git fact.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_agent_auth::require_agent;
use talaria_body::{optional_max_string_member, parse, string_array_member, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_git_hosts::{self as hosts, GitHost};
use talaria_session::secretbox_or_500;
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return Ok(gate),
    };
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let repo = match string_member(obj, "repo", 3, 400) {
        Ok(r) => r,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The refs git handed the hook: full ref names, ≤200 chars each, ≤50 of
    // them (one push cannot honestly carry more).
    let refs = match string_array_member(obj, "refs", 1, 200, 0, 50) {
        Ok(r) => r,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The remote's host (`host[:port]`), which the rendered hook sends. A
    // hook rendered before hosts were named sends none, and the agent's
    // grants decide — unambiguous unless the repo is granted on two hosts.
    let authority = match optional_max_string_member(obj, "host", 253) {
        Ok(h) => h.map(|h| h.trim().to_lowercase()).filter(|h| !h.is_empty()),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    // No proven identity (the legacy org-wide key) or no grant: the same
    // answer as the credential route, for the same reason — a caller we
    // cannot scope is one we cannot answer for.
    let Some(agent_id) = caller.id.clone() else {
        tracing::warn!("[secrets] a legacy-key caller asked to push to {repo} — refused");
        return Ok(house_error(
            StatusCode::NOT_FOUND,
            "no credential for that host",
        ));
    };
    let host_id = match &authority {
        Some(a) => match hosts::host_row_at(&state.pg, a).await {
            Ok(row) => row.map(|r| r.id),
            Err(e) => return Ok(internal("[secrets] push check host read failed", e)),
        },
        None => hosts::resolve_grant(&state.pg, &agent_id, &repo, None)
            .await
            .ok()
            .map(|g| g.host_id),
    };
    let rule = match &host_id {
        Some(h) => hosts::repo_rule(&state.pg, &agent_id, h, &repo).await,
        None => None,
    };
    let Some(rule) = rule else {
        tracing::warn!(
            "[secrets] {} has no grant covering {repo} — push refused",
            caller.model
        );
        return Ok(house_error(
            StatusCode::NOT_FOUND,
            "no credential for that host",
        ));
    };

    // A null base_branch resolves to the repo's live default — one read of
    // its host per push, on the same login the push itself would spend.
    // Unresolvable means unanswerable: deny, with the operator line.
    let default_branch = match &rule.base_branch {
        Some(b) => b.clone(),
        None => {
            let sb = secretbox_or_500(&state, "[secrets] push check secretbox").await?;
            let resolved = match hosts::host(&state.pg, &sb, &rule.host_id).await {
                Ok(host) => host.default_branch(&repo).await.ok(),
                Err(_) => None,
            };
            match resolved {
                Some(b) => b,
                None => {
                    tracing::warn!(
                        "[secrets] could not resolve the default branch of {repo} — push refused"
                    );
                    return Ok(house_error(
                        StatusCode::BAD_GATEWAY,
                        "could not resolve the repo's default branch — try again",
                    ));
                }
            }
        }
    };

    for ref_name in &refs {
        if let Err(sentence) = hosts::push_allowed(&rule, &default_branch, ref_name) {
            tracing::warn!(
                "[secrets] {} push to {repo}:{ref_name} declined — {sentence}",
                caller.model
            );
            return Ok((
                StatusCode::FORBIDDEN,
                [(axum::http::header::CACHE_CONTROL, "no-store")],
                Json(json!({ "error": sentence })),
            )
                .into_response());
        }
    }
    tracing::warn!(
        "[secrets] {} push to {repo} ({} ref(s)) allowed by repo rules",
        caller.model,
        refs.len()
    );
    Ok((
        StatusCode::OK,
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(json!({ "ok": true })),
    )
        .into_response())
}
