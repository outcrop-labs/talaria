// /api/secrets/git-push-check.
//
// THE PRE-PUSH GATE — the platform's own answer to "whose call is main".
// GitHub's branch protection enforces the posture where the org's plan
// allows it; this route is what the rendered pre-push hook asks, and it
// works for every granted repo including the private ones a free plan
// cannot protect. The hook fires inside the container BEFORE any bytes
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
use talaria_body::{parse, string_array_member, string_member};
use talaria_error::{house_error, object_or_400};
use talaria_github as github;
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
    let Some(rule) = github::repo_rule(&state.pg, &agent_id, &repo).await else {
        tracing::warn!(
            "[secrets] {} has no grant covering {repo} — push refused",
            caller.model
        );
        return Ok(house_error(
            StatusCode::NOT_FOUND,
            "no credential for that host",
        ));
    };

    // A null base_branch resolves to the repo's live default — one GitHub
    // read per push, on the same installation token the push itself would
    // spend. Unresolvable means unanswerable: deny, with the operator line.
    let default_branch = match &rule.base_branch {
        Some(b) => b.clone(),
        None => {
            let sb = secretbox_or_500(&state, "[secrets] push check secretbox").await?;
            match github::repo_default_branch(&state.pg, &sb, &repo).await {
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
        if let Err(sentence) = github::push_allowed(&rule, &default_branch, ref_name) {
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
