// /api/rag/collections — port of ui/src/routes/api/rag.collections.ts. The
// RAG collection registry. GET → every collection + its access bindings (the
// two auto ones ensured first; members get the picker shape with the binding
// matrix blanked — that matrix is admin governance). POST → spin up a custom
// collection. The write is admin-only; any signed-in user reads.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use serde_json::json;

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    array_msg, array_too_big_msg, as_object, enum_member, object_msg, optional_max_string_member,
    parse, present_nullable_max_string_member, string_member, zod_type_name,
};
use crate::error::house_error;
use crate::retrieval::collections::{self, AccessBinding, RagCollection};
use crate::retrieval::{embed, qdrant};
use crate::session::{actor_of, require_admin, require_user};
use crate::state::AppState;

/// `timestamptz::text` ("2026-08-29 04:49:51.123456+00") → the ISO-milliseconds
/// string a JS Date stringifies to. The fold runs through epoch millis, so
/// the microseconds the text carries and the Date both land on the same
/// millisecond. An unparseable string passes through — the column renders one
/// fixed format; anything else means the row was hand-edited, and the honest
/// answer is what is actually stored.
pub(crate) fn pg_text_to_iso(s: &str) -> String {
    // `%#z` not `%:z`: Postgres prints the offset hour-only when its minutes
    // are zero (`+00`) and `+HH:MM` otherwise; the flag admits both shapes.
    chrono::DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f%#z")
        .map(|dt| crate::agent_auth::epoch_ms_to_iso(dt.timestamp_millis()))
        .unwrap_or_else(|_| s.to_string())
}

/// The row as the TS select aliases it — key order included, because the
/// spread lands on the wire byte-for-byte. No `bindings` key: the TS row
/// itself carries none (createCollection returns the bare row; the GET
/// attaches them, see below).
pub(crate) fn row_json(col: &RagCollection) -> Value {
    json!({
        "id": col.id,
        "name": col.name,
        "kind": col.kind,
        "qdrantName": col.qdrant_name,
        "description": col.description,
        "auto": col.auto,
        "createdBy": col.created_by,
        "createdAt": pg_text_to_iso(&col.created_at),
        "schemaVersion": col.schema_version,
    })
}

/// z.array(Binding).max(200): elements validate BEFORE the array-length check
/// (zod 4's issue order — the same probed behavior the workflows arrays are
/// pinned on). Unknown keys inside an element strip silently; a nullish
/// principalId and an absent one are the same value on the wire and in the
/// table.
pub(crate) fn parse_bindings(v: Option<&Value>) -> Result<Option<Vec<AccessBinding>>, String> {
    let Some(v) = v else {
        return Ok(None); // absent — what POST's `.optional()` admits
    };
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    let mut out = Vec::with_capacity(arr.len());
    for el in arr {
        let inner = el
            .as_object()
            .ok_or_else(|| object_msg(zod_type_name(el)))?;
        let principal_type =
            enum_member(inner, "principalType", &["all", "user", "agent", "team"])?;
        let principal_id = present_nullable_max_string_member(inner, "principalId", 200)?.flatten();
        out.push(AccessBinding {
            principal_type,
            principal_id,
        });
    }
    if arr.len() > 200 {
        return Err(array_too_big_msg(200));
    }
    Ok(Some(out))
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // The two auto collections exist before the list is read — errors
    // swallowed exactly as TS's `.catch(() => {})`: a dead Qdrant must not
    // take the registry down with it.
    let qd = qdrant::real_deps();
    let ed = embed::real_deps();
    let _ = collections::ensure_auto_collections(&state.pg, &qd, &ed).await;
    let list = match collections::list_collections(&state.pg).await {
        Ok(l) => l,
        Err(_) => return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    };
    // Members get names only — the doc "Brain" picker. Key order is the TS
    // literal's, and `bindings` is an EMPTY array, not omitted.
    if user.role != "admin" {
        let rows: Vec<Value> = list
            .iter()
            .map(|(c, _)| {
                json!({
                    "id": c.id, "name": c.name, "kind": c.kind,
                    "description": c.description, "auto": c.auto, "bindings": [],
                })
            })
            .collect();
        return Json(json!({ "collections": rows })).into_response();
    }
    // The access rows carry their own collectionId in TS (the filter runs on
    // the raw select), so each binding on the wire has it too.
    let rows: Vec<Value> = list
        .iter()
        .map(|(c, bs)| {
            let mut row = row_json(c);
            row.as_object_mut()
                .expect("row_json builds an object")
                .insert(
                    "bindings".to_string(),
                    Value::Array(
                        bs.iter()
                            .map(|b| {
                                json!({
                                    "collectionId": c.id,
                                    "principalType": b.principal_type,
                                    "principalId": b.principal_id,
                                })
                            })
                            .collect(),
                    ),
                );
            row
        })
        .collect();
    Json(json!({ "collections": rows })).into_response()
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
    let name = match string_member(obj, "name", 2, 80) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let description = match optional_max_string_member(obj, "description", 500) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let bindings = match parse_bindings(obj.get("bindings")) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let created_by = user
        .email
        .as_deref()
        .or(user.name.as_deref())
        .unwrap_or("admin");
    let bindings_ref = bindings.as_deref();
    let qd = qdrant::real_deps();
    let ed = embed::real_deps();
    let col = match collections::create_collection(
        &state.pg,
        &qd,
        &ed,
        &collections::CreateCollection {
            name: &name,
            description: description.as_deref(),
            created_by,
            bindings: bindings_ref,
        },
    )
    .await
    {
        Ok(c) => c,
        // TS's catch: the create's own sentence (a down embedding service, a
        // Qdrant that will not build the collection) IS the answer, at 400.
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let (pg, actor, target_id, target_label) = (
        state.pg.clone(),
        actor_of(&user),
        col.id.clone(),
        col.name.clone(),
    );
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "rag.create",
                target_type: "rag_collection",
                target_id: Some(&target_id),
                target_label: Some(&target_label),
                before: None,
                after: None,
            },
        )
        .await;
    });
    Json(json!({ "collection": row_json(&col) })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::too_big_msg;

    #[test]
    fn timestamps_fold_to_the_js_date_string() {
        assert_eq!(
            pg_text_to_iso("2026-08-29 04:49:51.123456+00"),
            "2026-08-29T04:49:51.123Z"
        );
        // Millisecond precision and a non-UTC offset render the same instant.
        assert_eq!(
            pg_text_to_iso("2026-01-02 03:04:05.6+00"),
            "2026-01-02T03:04:05.600Z"
        );
        assert_eq!(
            pg_text_to_iso("2026-01-02 05:04:05.600+02"),
            "2026-01-02T03:04:05.600Z"
        );
        // Anything else passes through as stored.
        assert_eq!(pg_text_to_iso("hand-edited"), "hand-edited");
    }

    #[test]
    fn bindings_parse_the_zod_order() {
        // Absent is None (POST's optional); elements before the length bound.
        assert_eq!(parse_bindings(None), Ok(None));
        let over: Vec<Value> = (0..201).map(|_| json!({"principalType": "user"})).collect();
        assert_eq!(
            parse_bindings(Some(&Value::Array(over))),
            Err(array_too_big_msg(200))
        );
        // A bad element outranks the over-long array it sits in.
        let bad = [
            json!({"principalType": "nope"}),
            json!({"principalType": "user"}),
        ];
        assert_eq!(
            parse_bindings(Some(&Value::Array(bad.to_vec()))),
            Err(enum_member(
                bad[0].as_object().unwrap(),
                "principalType",
                &["all", "user", "agent", "team"],
            )
            .unwrap_err())
        );
        // A non-object element is the object message; nullish and absent
        // principalId are the same None; unknown keys strip.
        assert_eq!(
            parse_bindings(Some(&json!(["x"]))),
            Err(object_msg("string"))
        );
        let ok = json!([
            {"principalType": "all"},
            {"principalType": "user", "principalId": null},
            {"principalType": "team", "principalId": "t1", "extra": true},
        ]);
        assert_eq!(
            parse_bindings(Some(&ok)),
            Ok(Some(vec![
                AccessBinding {
                    principal_type: "all".into(),
                    principal_id: None
                },
                AccessBinding {
                    principal_type: "user".into(),
                    principal_id: None
                },
                AccessBinding {
                    principal_type: "team".into(),
                    principal_id: Some("t1".into())
                },
            ]))
        );
        // The nullish max: a 201-unit id is the string sentence.
        let long = "x".repeat(201);
        assert_eq!(
            parse_bindings(Some(
                &json!([{"principalType": "user", "principalId": long}])
            )),
            Err(too_big_msg(200))
        );
    }
}
