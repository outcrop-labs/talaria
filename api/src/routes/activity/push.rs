// /api/push — the browser's half of the closed-tab plane. GET /key hands the
// instance's VAPID public key (what pushManager.subscribe wants as
// applicationServerKey); POST /subscribe files a device browser's
// subscription (the push service's endpoint plus the two key halves RFC 8291
// encrypts against); POST /unsubscribe retires one. The server's half —
// keypair custody, encryption, delivery, pruning — is src/push.rs; this file
// is only the door.
//
// The endpoint is validated to an https URL because the push services'
// own contract says so (an http "endpoint" is not a push service, it is a
// request to mail ciphertext to a wire anyone can read). The two key halves
// are length-checked at the door too — a row that can never decrypt is dead
// weight the delivery loop would only prune later.

use crate::body::{as_object, parse, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::push::vapid_keys;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde_json::{Value, json};

/// The wire cap on an endpoint URL: push-service endpoints run a few hundred
/// chars; 2048 is url_member's own ceiling shape and never in play.
const ENDPOINT_MAX: usize = 2048;
/// Key-half wire caps — the browser sends ~87 chars (p256dh) and ~22 (auth);
/// 512 admits every legal padding/spelling with room to spare.
const KEY_MAX: usize = 512;

/// GET /api/push/key — the instance's VAPID public key, base64url of the
/// 65-byte uncompressed point: the exact string the client decodes into
/// applicationServerKey. Behind require_user because the only caller is a
/// signed-in browser mid-subscribe; there is nothing secret in the PUBLIC
/// half, but the door costs nothing and answers nobody else.
pub async fn key(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_user(&state, &headers).await {
        return gate;
    }
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[push/key] the secretbox did not load: {e}");
            return thrown_internal_error();
        }
    };
    let keys = match vapid_keys(&state.pg, &sb).await {
        Ok(k) => k,
        Err(e) => {
            tracing::error!("[push/key] could not produce the vapid keypair: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({
        "publicKey": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(keys.public),
    }))
    .into_response()
}

/// The validated subscribe body.
#[derive(Debug)]
struct Subscribe {
    endpoint: String,
    p256dh: String,
    auth: String,
}

/// The browser's PushSubscriptionJSON shape: `{ endpoint, keys: { p256dh,
/// auth } }`. Checks run in schema order — endpoint (string, then its https
/// refine), keys (record), p256dh, auth — and the key halves are decoded and
/// length-checked here so a row that can never decrypt never files. Messages
/// are pinned in the test at the bottom of this file.
fn validate_subscribe(obj: &serde_json::Map<String, Value>) -> Result<Subscribe, String> {
    let endpoint = string_member(obj, "endpoint", 1, ENDPOINT_MAX)?;
    // The https refine: scheme-checked, not just parseable — see the header.
    let https = url::Url::parse(&endpoint)
        .map(|u| u.scheme() == "https")
        .unwrap_or(false);
    if !https {
        return Err("Invalid URL".into());
    }
    // keys: a record — with zod's own received-name per shape, so an array
    // in the slot says "array", an absent one says "undefined".
    let keys = match obj.get("keys") {
        None => return Err(crate::body::object_msg("undefined")),
        Some(v) => match v.as_object() {
            Some(m) => m,
            None => return Err(crate::body::object_msg(crate::body::zod_type_name(v))),
        },
    };
    let p256dh = string_member(keys, "p256dh", 1, KEY_MAX)?;
    let auth = string_member(keys, "auth", 1, KEY_MAX)?;
    // Length at the door, spelling at delivery: the browser may pad or not,
    // so either alphabet decodes — but only 65 (point) and 16 (auth secret)
    // bytes can ever drive RFC 8291.
    let point = decode_key_half(&p256dh)?;
    if point.len() != 65 {
        return Err("Invalid subscription keys".into());
    }
    let secret = decode_key_half(&auth)?;
    if secret.len() != 16 {
        return Err("Invalid subscription keys".into());
    }
    Ok(Subscribe {
        endpoint,
        p256dh,
        auth,
    })
}

/// b64url first (what browsers actually send), standard b64 as the fallback —
/// the same forgiving decode delivery uses, applied before the row files.
fn decode_key_half(s: &str) -> Result<Vec<u8>, String> {
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
    URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .or_else(|_| STANDARD.decode(s.as_bytes()))
        .map_err(|_| "Invalid subscription keys".to_string())
}

/// POST /api/push/subscribe — file (or re-file) one device browser. Upsert
/// on the endpoint: a browser that re-subscribes (service worker refresh,
/// push service rotation, a second login) overwrites its own row, and a row
/// whose endpoint moved to another account is CLAIMED — one endpoint is one
/// browser, whoever is signed into it now.
pub async fn subscribe(
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
    let sub = match validate_subscribe(obj) {
        Ok(s) => s,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(e) = sqlx::query(
        "insert into push_subscriptions (user_id, endpoint, p256dh, auth) \
         values ($1::uuid, $2, $3, $4) \
         on conflict (endpoint) do update \
         set user_id = excluded.user_id, p256dh = excluded.p256dh, \
             auth = excluded.auth, last_seen_at = now()",
    )
    .bind(&user.id)
    .bind(&sub.endpoint)
    .bind(&sub.p256dh)
    .bind(&sub.auth)
    .execute(&state.pg)
    .await
    {
        tracing::error!("[push/subscribe] the subscription write failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}

/// POST /api/push/unsubscribe — retire the caller's own subscription for one
/// endpoint (the settings toggle, or the browser's own unsubscribe event
/// relayed by the client). Scoped to the caller: an endpoint alone is not
/// proof of ownership, the pair is. Absent rows answer ok — unsubscribing
/// twice is the subscriber's right, not an error.
pub async fn unsubscribe(
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
    let endpoint = match string_member(obj, "endpoint", 1, ENDPOINT_MAX) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(e) =
        sqlx::query("delete from push_subscriptions where user_id = $1::uuid and endpoint = $2")
            .bind(&user.id)
            .bind(&endpoint)
            .execute(&state.pg)
            .await
    {
        tracing::error!("[push/unsubscribe] the subscription delete failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::json;

    fn obj(v: Value) -> serde_json::Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    // The subscribe door's message table, in check order: endpoint first
    // (type, then the https refine), then keys the record, then the halves.
    // The good key halves are BUILT, not pasted — a 0x04-prefixed 65-byte
    // point and a 16-byte secret are what the lengths below pin, and a
    // pasted string could only prove it looks like one.
    #[test]
    fn subscribe_validation_is_pinned() {
        let point = {
            let mut v = vec![0x04u8]; // uncompressed SEC1 prefix
            v.extend_from_slice(&[0x42u8; 64]); // X ‖ Y
            v
        };
        let good_keys = json!({
            "p256dh": URL_SAFE_NO_PAD.encode(point),
            "auth": URL_SAFE_NO_PAD.encode([0x07u8; 16]),
        });
        // The full legal shape passes — 65 bytes of point, 16 of auth.
        let ok = obj(json!({
            "endpoint": "https://push.example.example/s/abc",
            "keys": good_keys,
        }));
        assert!(validate_subscribe(&ok).is_ok());

        // endpoint: not a string; not a URL; not https.
        let not_string = obj(json!({ "endpoint": 7, "keys": good_keys }));
        assert_eq!(
            validate_subscribe(&not_string).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        let not_url = obj(json!({
            "endpoint": "not a url at all",
            "keys": good_keys,
        }));
        assert_eq!(validate_subscribe(&not_url).unwrap_err(), "Invalid URL");
        let not_https = obj(json!({
            "endpoint": "http://push.example.example/s/abc",
            "keys": good_keys,
        }));
        assert_eq!(validate_subscribe(&not_https).unwrap_err(), "Invalid URL");

        // keys: absent, and not a record.
        let no_keys = obj(json!({ "endpoint": "https://push.example.example/s/abc" }));
        assert_eq!(
            validate_subscribe(&no_keys).unwrap_err(),
            "Invalid input: expected object, received undefined"
        );
        let keys_array = obj(json!({
            "endpoint": "https://push.example.example/s/abc",
            "keys": [good_keys.clone()],
        }));
        assert_eq!(
            validate_subscribe(&keys_array).unwrap_err(),
            "Invalid input: expected object, received array"
        );

        // The halves: missing, undecodable, and the right decodings in the
        // wrong shapes — a 16-byte auth secret where the point belongs.
        let missing_auth = obj(json!({
            "endpoint": "https://push.example.example/s/abc",
            "keys": { "p256dh": good_keys["p256dh"] },
        }));
        assert!(validate_subscribe(&missing_auth).is_err());
        let garbage = obj(json!({
            "endpoint": "https://push.example.example/s/abc",
            "keys": { "p256dh": "!!not base64!!", "auth": good_keys["auth"] },
        }));
        assert_eq!(
            validate_subscribe(&garbage).unwrap_err(),
            "Invalid subscription keys"
        );
        let swapped = obj(json!({
            "endpoint": "https://push.example.example/s/abc",
            "keys": { "p256dh": good_keys["auth"], "auth": good_keys["auth"] },
        }));
        assert_eq!(
            validate_subscribe(&swapped).unwrap_err(),
            "Invalid subscription keys"
        );
    }
}
