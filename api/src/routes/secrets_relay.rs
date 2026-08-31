// /api/secrets/relay — port of ui/src/routes/api/secrets.relay.ts.
//
// HAND AN AGENT A CREDENTIAL, MID-CONVERSATION, WITHOUT PUTTING IT IN THE
// CHAT. The paste this exists to prevent is the ordinary one: somebody needs
// their agent to do a thing that takes a token, so they type the token into
// the message box — and then it is in the transcript, in the database, in
// the prompt of every subsequent turn, and on its way to whichever provider
// serves the next reply. This is the SAME gesture with the value routed
// around the conversation: what goes back to the composer is a NAME, and
// there is no path by which the value becomes a message.
//
// NOT AN ADMIN ROUTE, deliberately — the observed alternative to a friction
// like "file a ticket to hand your assistant a key" is pasting the key into
// the chat. What bounds it instead is the same gate chat itself uses: if you
// may drive this agent, you may hand it a credential for one errand — and
// only this agent, only once, only for the next hour.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    as_object, nullish_max_string_member, optional_string_array_member, parse, string_member,
};
use crate::error::house_error;
use crate::fleet::usable_agent_gate;
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use crate::workspace_secrets::mint_relay;

struct RelayBody {
    agent_model: String,
    label: String,
    value: String,
    note: Option<String>,
    allowed_hosts: Option<Vec<String>>,
}

fn parse_body(obj: &serde_json::Map<String, serde_json::Value>) -> Result<RelayBody, String> {
    Ok(RelayBody {
        agent_model: string_member(obj, "agentModel", 1, 120)?,
        label: string_member(obj, "label", 1, 60)?,
        // Bounded like the vault's: a PEM private key is a few thousand
        // characters, and a limit that rejects one pushes somebody to paste
        // it somewhere worse.
        value: string_member(obj, "value", 1, 20_000)?,
        note: nullish_max_string_member(obj, "note", 400)?,
        // Optional: pin the one-shot to the host it is for — the only bound
        // that survives the agent being talked into spending it elsewhere.
        allowed_hosts: optional_string_array_member(obj, "allowedHosts", 0, 253, 10)?,
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let body = match parse_body(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // The same owner-aware gate `/api/chat` applies before letting a turn
    // through — which is the point: this cannot reach an agent the caller
    // could not have talked to anyway.
    let gate = match usable_agent_gate(&state.pg, &user.id, &user.role).await {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("[secrets.relay] gate read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    if !gate(&body.agent_model) {
        return house_error(StatusCode::FORBIDDEN, "forbidden: no access to this agent");
    }

    let actor = actor_of(&user);
    // A failed secretbox is the mint failing — same sentence, same status,
    // nothing about the box's internals to the caller.
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[secrets.relay] mint failed: {e}");
            return house_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not mint that one-shot — see server logs",
            );
        }
    };
    let relay = match mint_relay(
        &state.pg,
        &sb,
        &body.label,
        &body.value,
        &body.agent_model,
        Some(&actor),
        body.note.clone(),
        body.allowed_hosts.clone(),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            // Never echo the raw error to the caller: this path sits one
            // variable away from the value it was handed.
            tracing::error!("[secrets.relay] mint failed: {e}");
            return house_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not mint that one-shot — see server logs",
            );
        }
    };

    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor,
            action: "secrets.relay",
            target_type: "secret",
            target_id: Some(&relay.name),
            target_label: None,
            before: None,
            after: Some(json!({
                "agentModel": body.agent_model,
                "label": relay.label,
                "expiresAt": relay.expires_at,
                "uses": 1,
                "allowedHosts": relay.allowed_hosts,
            })),
        },
    )
    .await;
    // THE HANDLE, AND NOTHING THAT COULD RECONSTRUCT THE VALUE. There is no
    // read path anywhere in this feature that returns one, and this is not
    // the first.
    Json(json!({
        "handle": relay.handle,
        "name": relay.name,
        "label": relay.label,
        "expiresAt": relay.expires_at,
    }))
    .into_response()
}
