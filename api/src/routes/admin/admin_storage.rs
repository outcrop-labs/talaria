// /api/admin/storage — port of ui/src/routes/api/admin.storage.ts. Object
// storage (uploads blob store) config. GET → config (secrets masked) + blob
// stats + migration/sync status + the built-in bucket's endpoint. PUT → save
// config. POST → connection tests, local→bucket migration, or a full sync to
// the replica.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, enum_member, object_msg, optional_enum_member, parse, zod_type_name};
use crate::error::{house_error, thrown_internal_error};
use crate::secretbox::SecretBox;
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use crate::storage::{
    BucketTarget, StorageConfig, ensure_bucket, get_storage_config, internal_target,
    public_storage_config, refuse_dev_secret, replica_target, set_storage_config, test_storage,
};
use crate::uploads::{
    migrate_status, migrate_uploads_to_s3, sync_status, sync_uploads_to_replica, upload_stats,
};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

async fn secretbox_or_500(state: &AppState) -> Result<SecretBox, Response> {
    state.secretbox().await.map_err(|e| {
        tracing::error!("[admin/storage] secretbox unavailable: {e}");
        thrown_internal_error()
    })
}

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let sb = match secretbox_or_500(&state).await {
        Ok(sb) => sb,
        Err(r) => return r,
    };
    let config = public_storage_config(&state.pg, &sb).await;
    let stats = upload_stats(&state.pg).await;
    let migrate = migrate_status(&state.pg).await;
    let sync = sync_status(&state.pg).await;
    let internal = internal_target();
    Json(serde_json::json!({
        "config": config,
        "stats": stats,
        "migrate": migrate,
        "sync": sync,
        "internal": { "endpoint": internal.endpoint, "bucket": internal.bucket },
    }))
    .into_response()
}

/// One Target block (the five required S3 fields + the optional secret) —
/// `Target` in admin.storage.ts, every rejection in zod's own words. The
/// prefix's `regex(/^$|^[a-zA-Z0-9._/-]+\/$/)` carries its custom sentence.
fn target_of(o: &serde_json::Map<String, Value>) -> Result<BucketTarget, String> {
    use crate::body::{boolean_member, optional_max_string_member, string_member};
    let endpoint = string_member(o, "endpoint", 0, 300)?;
    let region = string_member(o, "region", 0, 60)?;
    let bucket = string_member(o, "bucket", 0, 200)?;
    let access_key_id = string_member(o, "accessKeyId", 0, 200)?;
    // Omitted or empty = keep the currently stored secret — resolved by
    // the caller, who can see the current config.
    let secret_access_key =
        optional_max_string_member(o, "secretAccessKey", 400)?.unwrap_or_default();
    let path_style = boolean_member(o, "pathStyle")?;
    let prefix = string_member(o, "prefix", 0, 120)?;
    let prefix_ok = prefix.is_empty()
        || (prefix.ends_with('/')
            && prefix[..prefix.len() - 1]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/')));
    if !prefix_ok {
        return Err("prefix must end with /".into());
    }
    Ok(BucketTarget {
        endpoint,
        region,
        bucket,
        access_key_id,
        secret_access_key,
        path_style,
        prefix,
    })
}

/// `.trim().replace(/\/+$/, '')` — strip whitespace, then EVERY trailing slash.
fn trim_endpoint(mut e: String) -> String {
    e = e.trim().to_string();
    while e.ends_with('/') {
        e.pop();
    }
    e
}

pub async fn put(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let sb = match secretbox_or_500(&state).await {
        Ok(sb) => sb,
        Err(r) => return r,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Body key order: mode, the Target spread, replica — mode's enum message
    // is the one the harness pinned.
    let mode = match enum_member(obj, "mode", &["local", "internal", "s3"]) {
        Ok(m) => m,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let target = match target_of(obj) {
        Ok(t) => t,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let (replica_obj, replica_enabled) = match obj.get("replica") {
        None => return house_error(StatusCode::BAD_REQUEST, &object_msg("undefined")),
        Some(v) => match v.as_object() {
            Some(o) => match crate::body::boolean_member(o, "enabled") {
                Ok(b) => (o, b),
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            },
            None => return house_error(StatusCode::BAD_REQUEST, &object_msg(zod_type_name(v))),
        },
    };
    let replica = match target_of(replica_obj) {
        Ok(t) => t,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    let current = get_storage_config(&state.pg, &sb).await;
    // `body.secretAccessKey || current.secretAccessKey` — JS ||: the blank
    // secret on a local config is "keep what's stored", never "erase".
    let mut target = target;
    target.endpoint = trim_endpoint(target.endpoint);
    if target.secret_access_key.is_empty() {
        target.secret_access_key = current.target.secret_access_key.clone();
    }
    let mut replica = replica;
    replica.endpoint = trim_endpoint(replica.endpoint);
    if replica.secret_access_key.is_empty() {
        replica.secret_access_key = current.replica.secret_access_key.clone();
    }
    let next = StorageConfig {
        mode: mode.to_string(),
        target,
        replica,
        replica_enabled,
    };
    set_storage_config(&state.pg, &sb, &next).await;
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "settings.storage",
            target_type: "settings",
            target_id: None,
            target_label: None,
            before: None,
            after: Some(serde_json::json!({
                "mode": next.mode,
                "endpoint": next.target.endpoint,
                "bucket": next.target.bucket,
                "replica": if next.replica_enabled { next.replica.bucket.clone() } else { "off".into() },
            })),
        },
    )
    .await;
    let config = public_storage_config(&state.pg, &sb).await;
    Json(serde_json::json!({ "config": config })).into_response()
}

pub async fn post(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let sb = match secretbox_or_500(&state).await {
        Ok(sb) => sb,
        Err(r) => return r,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // z.object({ action: z.enum([...]).optional() }) — absent action is the
    // no-op branch, anything else answers the enum's own message.
    let action =
        match optional_enum_member(obj, "action", &["test", "test-replica", "migrate", "sync"]) {
            Ok(a) => a,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
    let cfg = get_storage_config(&state.pg, &sb).await;
    let audit = |action: &str, after: Option<Value>| {
        // fire-and-forget, exactly the TS `void logAudit(...)`
        let pg = state.pg.clone();
        let actor = actor_of(&user);
        let action = format!("storage.{action}");
        tokio::spawn(async move {
            log_audit(
                &pg,
                AuditEntry {
                    actor: &actor,
                    action: &action,
                    target_type: "settings",
                    target_id: None,
                    target_label: None,
                    before: None,
                    after,
                },
            )
            .await;
        });
    };
    // TS wraps every action in one try/catch → 400 {error}; the Rust halves
    // return Result<_, String>, mapped to the same shape.
    match action.as_deref() {
        Some("test") => {
            // Test what the current mode would actually use.
            let result: Result<Value, String> = if cfg.mode == "internal" {
                let t = internal_target();
                match refuse_dev_secret(&t) {
                    Ok(()) => match ensure_bucket(&t).await {
                        Ok(()) => Ok(test_storage(&t).await),
                        Err(msg) => Err(msg),
                    },
                    Err(msg) => Err(msg),
                }
            } else {
                Ok(test_storage(&cfg.target).await)
            };
            match result {
                Ok(v) => Json(v).into_response(),
                Err(msg) => house_error(StatusCode::BAD_REQUEST, &msg),
            }
        }
        Some("test-replica") => {
            let replica = replica_target(&cfg).unwrap_or_else(|| cfg.replica.clone());
            Json(test_storage(&replica).await).into_response()
        }
        Some("migrate") => match migrate_uploads_to_s3(&state.pg, &sb).await {
            Ok(status) => {
                audit(
                    "migrate",
                    Some(
                        serde_json::json!({ "total": status.get("total").cloned().unwrap_or(Value::Null) }),
                    ),
                );
                Json(serde_json::json!({ "migrate": status })).into_response()
            }
            Err(msg) => house_error(StatusCode::BAD_REQUEST, &msg),
        },
        Some("sync") => match sync_uploads_to_replica(&state.pg, &sb).await {
            Ok(status) => {
                audit(
                    "sync",
                    Some(
                        serde_json::json!({ "total": status.get("total").cloned().unwrap_or(Value::Null) }),
                    ),
                );
                Json(serde_json::json!({ "sync": status })).into_response()
            }
            Err(msg) => house_error(StatusCode::BAD_REQUEST, &msg),
        },
        _ => house_error(StatusCode::BAD_REQUEST, "unknown action"),
    }
}
