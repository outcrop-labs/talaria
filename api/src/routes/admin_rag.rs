// /api/admin/rag — port of ui/src/routes/api/admin.rag.ts. The retrieval
// console. GET → services health + both repair runs' projections + the
// upgrade status + reranker providers/config + KB-space brain bindings.
// PUT → reranker config and/or a space↔brain binding. POST → kick a repair
// run (detached), or { models, key? } → a provider's live model catalog.
// Admins only.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::json;
use sqlx::Row;

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    NumKind, as_object, nullable_uuid_member, optional_enum_member, optional_number_member, parse,
    present_nullable_max_string_member, too_big_msg, too_small_msg, utf16_len, uuid_member,
    zod_type_name,
};
use crate::error::house_error;
use crate::retrieval::backfill::{backfill_status, rag_health};
use crate::retrieval::embed;
use crate::retrieval::migrate::{reindex_status, retrieval_upgrade_status};
use crate::retrieval::qdrant;
use crate::retrieval::rerank::{self, RERANK_PROVIDERS, RerankPatch, providers_public};
use crate::retrieval::sources::resync_space_docs;
use crate::runs::defs::reindex::{start_backfill, start_reindex};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;

/// zod's nested-object refusal, for the two PATCH shapes that take one:
/// `Invalid input: expected object, received <type>`.
fn nested_object(
    v: &serde_json::Value,
) -> Result<&serde_json::Map<String, serde_json::Value>, String> {
    v.as_object().ok_or_else(|| {
        format!(
            "Invalid input: expected object, received {}",
            zod_type_name(v)
        )
    })
}

#[derive(Serialize)]
struct SpaceRow {
    id: String,
    name: String,
    #[serde(rename = "collectionId")]
    collection_id: Option<String>,
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let qd = qdrant::real_deps();
    let ed = embed::real_deps();
    let health = rag_health(&qd, &ed).await;
    let backfill = backfill_status(&state.pg).await;
    // The one soft-failing member of the GET: a dead Qdrant reads as a null
    // upgrade block (the health block above is the alarm for that), not as a
    // 500 that would take the whole console down with it. TS's `.catch(() =>
    // null)`.
    let upgrade = retrieval_upgrade_status(&state.pg, &qd, &ed).await.ok();
    let reindex = reindex_status(&state.pg).await;
    let config = rerank::rerank_config_public(&state.pg).await;
    let spaces: Vec<SpaceRow> = match sqlx::query(
        "select id::text, name, rag_collection_id::text as \"collectionId\" \
         from kb_spaces order by name asc",
    )
    .fetch_all(&state.pg)
    .await
    {
        Ok(rows) => rows
            .iter()
            .map(|r| SpaceRow {
                id: r.try_get::<String, _>("id").unwrap_or_default(),
                name: r.try_get::<String, _>("name").unwrap_or_default(),
                collection_id: r
                    .try_get::<Option<String>, _>("collectionId")
                    .unwrap_or(None),
            })
            .collect(),
        Err(_) => return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    };
    Json(json!({
        "health": health,
        "backfill": backfill,
        "upgrade": upgrade,
        "reindex": reindex,
        "rerank": { "providers": providers_public(), "config": config },
        "spaces": spaces,
    }))
    .into_response()
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // parseBody validates the WHOLE body before the handler runs its first
    // line — a valid reranker beside an invalid spaceBrain must write
    // NEITHER. Both sections parse into locals here; nothing touches a row
    // until both hold.
    let mut reranker: Option<RerankPatch> = None;
    if let Some(v) = obj.get("reranker") {
        let inner = match nested_object(v) {
            Ok(o) => o,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        // zod's enum list is 'off' plus the catalog, in catalog order.
        let mut ids: Vec<&str> = vec!["off"];
        ids.extend(RERANK_PROVIDERS.iter().map(|p| p.id));
        let provider = match optional_enum_member(inner, "provider", &ids) {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        let url = match present_nullable_max_string_member(inner, "url", 500) {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        let model = match present_nullable_max_string_member(inner, "model", 200) {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        let api_key = match present_nullable_max_string_member(inner, "apiKey", 500) {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        let candidates = match optional_number_member(inner, "candidates", NumKind::Int, 5.0, 100.0)
        {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        reranker = Some(RerankPatch {
            provider,
            url,
            model,
            api_key,
            candidates: candidates.map(|c| c as i64),
        });
    }

    // ── spaceBrain: bind a KB space to a custom collection; null unbinds.
    let mut space_brain: Option<(String, Option<String>)> = None;
    if let Some(v) = obj.get("spaceBrain") {
        let inner = match nested_object(v) {
            Ok(o) => o,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        let space_id = match uuid_member(inner, "spaceId") {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        let collection_id = match nullable_uuid_member(inner, "collectionId") {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        space_brain = Some((space_id, collection_id));
    }

    // ── apply ──
    if let Some(patch) = reranker {
        match rerank::set_rerank_config(&state, patch).await {
            Ok(next) => {
                // The audit after is {provider, model} with model dropped
                // when the row has none — TS's JSON.stringify skips
                // undefined keys.
                let mut after = serde_json::Map::new();
                if let Some(p) = next.get("provider").and_then(serde_json::Value::as_str) {
                    after.insert("provider".into(), json!(p));
                }
                if let Some(m) = next.get("model").and_then(serde_json::Value::as_str) {
                    after.insert("model".into(), json!(m));
                }
                let (pg, actor) = (state.pg.clone(), actor_of(&user));
                tokio::spawn(async move {
                    log_audit(
                        &pg,
                        AuditEntry {
                            actor: &actor,
                            action: "rag.reranker",
                            target_type: "rag",
                            target_id: Some("reranker"),
                            target_label: None,
                            before: None,
                            after: Some(serde_json::Value::Object(after)),
                        },
                    )
                    .await;
                });
            }
            // TS lets setRerankConfig throw to the runtime's generic 500.
            Err(_) => return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
        }
    }

    if let Some((space_id, collection_id)) = space_brain {
        let updated =
            sqlx::query("update kb_spaces set rag_collection_id = $2::uuid where id = $1::uuid")
                .bind(&space_id)
                .bind(&collection_id)
                .execute(&state.pg)
                .await;
        if updated.is_err() {
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
        // Existing docs move to their new home right away — detached, errors
        // swallowed exactly as TS's `.catch(() => {})`.
        {
            let (pg, space_id) = (state.pg.clone(), space_id.clone());
            tokio::spawn(async move {
                let qd = qdrant::real_deps();
                let ed = embed::real_deps();
                let _ = resync_space_docs(&pg, &qd, &ed, &space_id).await;
            });
        }
        let (pg, actor, after) = (
            state.pg.clone(),
            actor_of(&user),
            json!({ "collectionId": collection_id }),
        );
        tokio::spawn(async move {
            log_audit(
                &pg,
                AuditEntry {
                    actor: &actor,
                    action: "rag.space_brain",
                    target_type: "kb-space",
                    target_id: Some(&space_id),
                    target_label: None,
                    before: None,
                    after: Some(after),
                },
            )
            .await;
        });
    }

    Json(json!({ "rerank": { "config": rerank::rerank_config_public(&state.pg).await } }))
        .into_response()
}

// ── POST's union, decoded ────────────────────────────────────────────────────

/// z.union([actionObject, modelsObject]) resolved. The dispatch below was
/// probed against zod 4.3.6 itself, not reasoned about: the ACTION arm wins
/// whenever it parses (extras strip — a models key beside a valid action is
/// ignored); the MODELS arm surfaces its string-check sentences ("Too
/// small"/"Too big") but only once every TYPE check passes — a non-string
/// models, or a key that is neither string nor null, aborts the branch and
/// the union answers its own generic sentence instead, even when a check
/// ALSO failed. Checks run in schema order: models before key.
#[derive(Debug, PartialEq)]
enum PostIntent {
    Kick(&'static str),
    Catalog {
        provider: String,
        key: Option<String>,
    },
}

fn classify_post(obj: &serde_json::Map<String, serde_json::Value>) -> Result<PostIntent, String> {
    // Arm 1: {action: enum(['reindex','backfill'])}.
    if let Some(serde_json::Value::String(s)) = obj.get("action")
        && (s == "reindex" || s == "backfill")
    {
        return Ok(PostIntent::Kick(if s == "reindex" {
            "reindex"
        } else {
            "backfill"
        }));
    }
    // Arm 2: {models: string(1..40), key?: string(<=500).nullish()}.
    let Some(serde_json::Value::String(provider)) = obj.get("models") else {
        return Err("Invalid input".into());
    };
    let key = match obj.get("key") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(k)) => Some(k.clone()),
        Some(_) => return Err("Invalid input".into()),
    };
    if provider.is_empty() {
        return Err(too_small_msg(1));
    }
    if utf16_len(provider) > 40 {
        return Err(too_big_msg(40));
    }
    if let Some(k) = &key
        && utf16_len(k) > 500
    {
        return Err(too_big_msg(500));
    }
    Ok(PostIntent::Catalog {
        provider: provider.clone(),
        key,
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    match classify_post(obj) {
        Ok(PostIntent::Kick(action)) => {
            // 'reindex' rebuilds collections in the current model's shape
            // then refills; 'backfill' refills in place. Both detach — the
            // response is "started", not "done", and the panel polls the
            // projections above. TS swallows a failed kick silently; a Start
            // button that quietly did nothing is the failure mode this plane
            // exists to end, so the one failure gets one warn line.
            let st = state.clone();
            tokio::spawn(async move {
                let res = if action == "reindex" {
                    start_reindex(&st).await
                } else {
                    start_backfill(&st).await
                };
                if let Err(e) = res {
                    tracing::warn!("[admin/rag] the {action} kick failed to start a run: {e}");
                }
            });
            let (pg, actor) = (state.pg.clone(), actor_of(&user));
            let audit_action = format!("rag.{action}");
            tokio::spawn(async move {
                log_audit(
                    &pg,
                    AuditEntry {
                        actor: &actor,
                        action: &audit_action,
                        target_type: "rag",
                        target_id: Some(action),
                        target_label: None,
                        before: None,
                        after: None,
                    },
                )
                .await;
            });
            Json(json!({ "started": true, "action": action })).into_response()
        }
        // The model-catalog arm. The candidate API key travels in a POST
        // body — NEVER a query string, where it would land in access/proxy
        // logs.
        Ok(PostIntent::Catalog { provider, key }) => {
            let http = crate::retrieval::real_http();
            // Catalog failures fall back to the documented list inside; only
            // a sealed key that cannot open errors, and TS lets that throw to
            // the runtime's generic 500.
            match rerank::rerank_models(&state, &http, &provider, key).await {
                Ok(models) => Json(json!({ "models": models })).into_response(),
                Err(_) => house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
            }
        }
        Err(msg) => house_error(StatusCode::BAD_REQUEST, &msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        match v {
            serde_json::Value::Object(m) => m,
            _ => unreachable!("the fixture is an object"),
        }
    }

    /// Every row is a probe ANSWER from zod 4.3.6 itself, captured before
    /// this table was written — the test pins the table to the oracle.
    #[test]
    fn the_post_union_dispatches_the_probed_zod_table() {
        // Arm 1 wins whenever it parses, extras stripped.
        assert_eq!(
            classify_post(&obj(json!({"action": "reindex"}))),
            Ok(PostIntent::Kick("reindex"))
        );
        assert_eq!(
            classify_post(&obj(json!({"action": "backfill", "models": ""}))),
            Ok(PostIntent::Kick("backfill"))
        );

        // Arm 2 happy paths: bare, nullish key, live key.
        assert_eq!(
            classify_post(&obj(json!({"models": "cohere"}))),
            Ok(PostIntent::Catalog {
                provider: "cohere".into(),
                key: None
            })
        );
        assert_eq!(
            classify_post(&obj(json!({"models": "cohere", "key": null}))),
            Ok(PostIntent::Catalog {
                provider: "cohere".into(),
                key: None
            })
        );
        assert_eq!(
            classify_post(&obj(json!({"models": "cohere", "key": "sk-x"}))),
            Ok(PostIntent::Catalog {
                provider: "cohere".into(),
                key: Some("sk-x".into())
            })
        );

        // Type failures are the union's generic sentence, wherever they sit —
        // including beside a check that ALSO fails (key's type aborts first).
        assert_eq!(classify_post(&obj(json!({}))), Err("Invalid input".into()));
        assert_eq!(
            classify_post(&obj(json!({"action": "bogus"}))),
            Err("Invalid input".into())
        );
        assert_eq!(
            classify_post(&obj(json!({"models": 123}))),
            Err("Invalid input".into())
        );
        assert_eq!(
            classify_post(&obj(json!({"models": "cohere", "key": 123}))),
            Err("Invalid input".into())
        );
        assert_eq!(
            classify_post(&obj(json!({"models": "", "key": 123}))),
            Err("Invalid input".into())
        );

        // Check failures surface their own sentences, in schema order:
        // models' bounds first, the key's after.
        assert_eq!(
            classify_post(&obj(json!({"models": ""}))),
            Err("Too small: expected string to have >=1 characters".into())
        );
        assert_eq!(
            classify_post(&obj(json!({"models": "x".repeat(41)}))),
            Err("Too big: expected string to have <=40 characters".into())
        );
        // An action that fails arm 1 on TYPE doesn't hide arm 2's check.
        assert_eq!(
            classify_post(&obj(json!({"action": 123, "models": ""}))),
            Err("Too small: expected string to have >=1 characters".into())
        );
        assert_eq!(
            classify_post(&obj(json!({"models": "", "key": "x".repeat(501)}))),
            Err("Too small: expected string to have >=1 characters".into())
        );
        assert_eq!(
            classify_post(&obj(json!({"models": "cohere", "key": "x".repeat(501)}))),
            Err("Too big: expected string to have <=500 characters".into())
        );
    }
}
