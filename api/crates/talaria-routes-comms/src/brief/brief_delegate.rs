// /api/brief/delegate. GET → the caller's live reply grants. POST
// { channelId, granted } → grant
// or revoke the assistant's reply-without-asking privilege, org-wide (null)
// or for one channel.
//
// OWNER-ONLY BY CONSTRUCTION. This is not an admin Perm — nobody can grant it
// on somebody else's behalf, and there is no route that takes a user id.
// `grant_reply` re-checks channel membership rather than trusting this
// handler, because a grant on a conversation the owner is not in would let an
// agent speak in a room its owner cannot read.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::{boolean_member, nullable_uuid_member};
use talaria_daily_brief::delegation::{grant_reply, list_grants, release_drafts, revoke_reply};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

/// The POST body: channelId (uuid or null) + granted.
struct DelegateBody {
    channel_id: Option<String>,
    granted: bool,
}

fn validate(obj: &serde_json::Map<String, serde_json::Value>) -> Result<DelegateBody, String> {
    // channelId must be PRESENT; null is the standing, org-wide grant.
    let channel_id = nullable_uuid_member(obj, "channelId")?;
    let granted = boolean_member(obj, "granted")?;
    Ok(DelegateBody {
        channel_id,
        granted,
    })
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(match list_grants(&state.pg, &user.id).await {
        Ok(grants) => Json(json!({ "grants": grants })).into_response(),
        Err(e) => internal("[brief] grant list failed", e),
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let body = match validate(obj) {
        Ok(b) => b,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    if !body.granted {
        return Ok(
            match revoke_reply(&state.pg, &user.id, body.channel_id.as_deref()).await {
                Ok(revoked) => Json(json!({ "revoked": revoked })).into_response(),
                Err(e) => internal("[brief] revoke failed", e),
            },
        );
    }
    let grant = match grant_reply(&state.pg, &user.id, body.channel_id.as_deref()).await {
        Ok(g) => g,
        Err(e) => return Ok(internal("[brief] grant failed", e)),
    };
    let Some(grant) = grant else {
        return Ok(house_error(
            StatusCode::FORBIDDEN,
            "That is not one of your conversations.",
        ));
    };
    // Granting permission to send a reply that is already written means
    // sending it — otherwise the control appears to do nothing until the other
    // person happens to speak again. A send failure here is swallowed to 0:
    // the grant itself stands, and the sweep will still draft — the response
    // must not read as a failed grant because one parked send hiccupped.
    let notify = talaria_notify::NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    let sent = release_drafts(&notify, &user.id, body.channel_id.as_deref())
        .await
        .unwrap_or(0);
    Ok(Json(json!({ "grant": grant, "sent": sent })).into_response())
}
