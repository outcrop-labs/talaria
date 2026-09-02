// /api/secrets/reveal.
//
// THE ONE ROUTE IN THIS FEATURE THAT RETURNS A CREDENTIAL. Everything else
// is built so a value has nowhere to come back through; this deliberate
// exception is what makes the store usable by the person who put the
// credential in it — a vault nobody can read from is a vault nobody uses,
// and the thing they use instead is a Slack thread. The narrowness is the
// design: one entry by key, only `revealable`, owner-or-shared (never
// admin), every look audited by the engine itself.
//
// POST rather than GET, and that is not ceremony: a GET returning a
// credential lands in browser history, in any proxy log that records paths,
// and in a referrer header. The body keeps it out of all three — and the
// headers below say NO CACHING out loud, because the default for a JSON
// response somebody fetches repeatedly is not obviously "never store this".

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::body::{as_object, parse, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use crate::workspace_secrets::reveal_entry;

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
    let name = match string_member(obj, "name", 0, 80) {
        Ok(n) => n,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let key = match string_member(obj, "key", 0, 40) {
        Ok(k) => k,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    let actor = actor_of(&user);
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[secrets] reveal failed: {e}");
            return thrown_internal_error();
        }
    };
    let out = match reveal_entry(&state.pg, &sb, &name, &key, &user.id, Some(&actor)).await {
        Ok(o) => o,
        Err(e) => {
            tracing::error!("[secrets] reveal failed: {e}");
            return thrown_internal_error();
        }
    };
    if out.value.is_none() {
        // The refusal reason goes to the CALLER here, unlike the agent
        // resolve path: a person asking for something they own or were shared
        // learns nothing from "not shared" they did not already know, and an
        // opaque failure on your own credential is how somebody concludes
        // the tool is broken and goes back to pasting keys into chat.
        let status = match out.refusal {
            "unknown" => StatusCode::NOT_FOUND,
            "not-shared" | "not-revealable" => StatusCode::FORBIDDEN,
            _ => StatusCode::CONFLICT,
        };
        return house_error(status, out.refusal);
    }

    // Belt and braces on the header that would otherwise carry the path of
    // this request to wherever the page navigates next.
    (
        [
            (
                header::CACHE_CONTROL,
                "no-store, no-cache, must-revalidate, private",
            ),
            (header::PRAGMA, "no-cache"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Json(json!({ "value": out.value })),
    )
        .into_response()
}
