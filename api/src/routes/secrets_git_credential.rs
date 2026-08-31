// /api/secrets/git-credential — port of ui/src/routes/api/secrets.git-credential.ts.
//
// THE SANDBOX'S WAY IN — where a handle could not otherwise reach. A handle
// substitutes at the MCP gateway, which covers every tool call an agent
// makes THROUGH Talaria. It does not cover the shell inside a workbench
// sandbox: a coding harness runs `git push` with its own bash tool, we are
// not in that path, and the handle goes out as literal text. So git asks us
// — its credential helper forwards the protocol, host and path here over
// the agent's own key, and what that buys is precise: the value never enters
// the model's context, never appears in command output, and is never
// written to disk. The model runs `git push`, and the push works.
//
// AGENT-AUTHENTICATED, NOT SESSION-AUTHENTICATED. The caller is a process
// in a container presenting the agent's own credential; a human's session
// cannot reach this route at all — there is nothing here a person needs
// that /api/secrets/reveal does not already do with an audit trail.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::agent_auth::require_agent;
use crate::body::{as_object, optional_max_string_member, parse, string_member};
use crate::error::house_error;
use crate::github;
use crate::state::AppState;
use crate::workspace_secrets::{HostCredential, credential_for_host};

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let host = match string_member(obj, "host", 1, 253) {
        Ok(h) => h,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let protocol = match optional_max_string_member(obj, "protocol", 20) {
        Ok(p) => p,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let path = match optional_max_string_member(obj, "path", 400) {
        Ok(p) => p,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // HTTPS ONLY. Answering for `http` would hand a live credential to a
    // cleartext connection, and a helper that does that turns one
    // misconfiguration into an interception.
    if let Some(p) = &protocol
        && p != "https"
    {
        tracing::warn!(
            "[secrets] {} asked for a {p} credential for {host} — refused",
            caller.model
        );
        return house_error(StatusCode::BAD_REQUEST, "https only");
    }

    // TWO SOURCES, workspace store first. A credential somebody deliberately
    // granted for this host is a more specific answer than the platform's
    // own GitHub token, and an operator who pinned one expects it used.
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[secrets] credential read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let mut cred = match credential_for_host(&state.pg, &sb, &caller.model, &host).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[secrets] credential read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    if cred.is_none()
        && let Some(agent_id) = &caller.id
    {
        // Talaria's GitHub installation token, scoped to a repo on THIS
        // agent's grant list — the credential a workbench job needs to push,
        // and the one that used to ride into the model's context inside the
        // clone URL. `id` is None for a LEGACY org-wide key, which names
        // nobody — and a caller we cannot identify is one we cannot scope to
        // a repo grant. The workspace store above already refuses those for
        // the same reason, so this only makes the two sources agree.
        match github::agent_git_credential(&state.pg, &sb, agent_id, &host, path.as_deref()).await {
            Ok(Some(g)) => {
                cred = Some(HostCredential {
                    username: g.username,
                    password: g.password,
                    name: format!("github:{}", g.repo),
                });
            }
            Ok(None) => {}
            Err(e) => {
                tracing::error!("[secrets] github credential failed: {e}");
                return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
            }
        }
    }
    let Some(cred) = cred else {
        // Reported to the OPERATOR, never elaborated to the caller: which
        // credentials exist and which hosts they cover is a map of the
        // workspace, and git only needs to know it got nothing.
        tracing::warn!(
            "[secrets] {} has no credential allowed for {host}",
            caller.model
        );
        return house_error(StatusCode::NOT_FOUND, "no credential for that host");
    };

    tracing::warn!(
        "[secrets] {} spent {} for {host} via git credential helper",
        caller.model,
        cred.name
    );
    (
        [
            (
                header::CACHE_CONTROL,
                "no-store, no-cache, must-revalidate, private",
            ),
            (header::PRAGMA, "no-cache"),
        ],
        Json(json!({ "username": cred.username, "password": cred.password })),
    )
        .into_response()
}
