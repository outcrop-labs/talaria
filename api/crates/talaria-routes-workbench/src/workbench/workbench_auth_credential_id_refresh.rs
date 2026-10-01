// POST /api/workbench/auth/v1/credential/{id}/refresh. The callback that makes
// Talaria the canonical refresher.
//
// A snapshot hands the harness an access token and `__remote__` where the
// refresh token would be, so when that token expires omp cannot renew it
// itself — it calls here. We open the stored credential, hand it to the omp
// auth bridge (which runs the provider's own refresher), re-seal the result,
// and answer with the redacted entry.
//
// A refusal the provider classifies as a dead grant is terminal: the
// credential is disabled with its reason so the snapshot stops serving it and
// the UI can say "sign in again" instead of the harness retrying for ever.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_coding_accounts as ca;
use talaria_error::{house_error, internal};
use talaria_state::AppState;

use super::coding_broker::{broker_agent, owned_account, snapshot_entry};

/// What a provider says when a grant is dead rather than merely stale. omp
/// classifies these the same way; a refresh that fails for any other reason
/// (a timeout, a 500 upstream) is transient and must NOT disable the row.
fn is_dead_grant(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("invalid_grant")
        || m.contains("invalid_request")
        || m.contains("unauthorized_client")
        || m.contains("expired_token")
}

// doc: Refresh one of the calling agent's coding-account credentials through
// doc: the provider's own refresher and re-seal it. The harness calls this
// doc: because the snapshot it holds carries no refresh token.
pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, Response> {
    let agent_id = broker_agent(&state, &headers).await?;
    let account = owned_account(&state, &agent_id, id).await?;
    if account.disabled {
        return Err(house_error(
            StatusCode::CONFLICT,
            "this credential is disabled; sign in again from Talaria",
        ));
    }
    // An api-key credential has nothing to refresh, and saying so is better
    // than handing the bridge something it will reject.
    if account.credential.get("type").and_then(|t| t.as_str()) != Some("oauth") {
        return Err(house_error(
            StatusCode::CONFLICT,
            "this credential is not refreshable",
        ));
    }

    let refreshed = match talaria_omp_auth::refresh(&account.provider, &account.credential).await {
        Ok(c) => c,
        Err(err) => {
            if is_dead_grant(&err) {
                let _ = ca::disable_account(&state.pg, id, &err).await;
                return Err(house_error(
                    StatusCode::CONFLICT,
                    &format!("{} — sign in again from Talaria", err),
                ));
            }
            return Err(house_error(StatusCode::BAD_GATEWAY, &err));
        }
    };

    let sb = state
        .secretbox()
        .await
        .map_err(|e| internal("[workbench/auth] secretbox failed", e))?;
    if let Err(err) = ca::store_refreshed(&state.pg, &sb, id, &refreshed).await {
        return Err(house_error(StatusCode::INTERNAL_SERVER_ERROR, &err));
    }

    let stored = ca::OpenAccount {
        id,
        provider: account.provider,
        identity_key: account.identity_key,
        credential: refreshed,
        disabled: false,
    };
    let Some(entry) = snapshot_entry(&state, &stored).await else {
        return Err(house_error(
            StatusCode::BAD_GATEWAY,
            "the refreshed credential is not servable",
        ));
    };
    Ok(axum::Json(json!({ "entry": entry })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dead_grant_is_told_apart_from_a_transient_failure() {
        assert!(is_dead_grant("token endpoint 400: invalid_grant"));
        assert!(is_dead_grant("INVALID_GRANT"));
        assert!(is_dead_grant("expired_token"));
        assert!(
            !is_dead_grant("request timed out"),
            "a timeout must not disable a working subscription"
        );
        assert!(!is_dead_grant("upstream 503"));
    }
}
