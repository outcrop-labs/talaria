// /api/history — port of ui/src/routes/api/history.ts. Version history for
// agent internals, one API over two stores:
//   snapshot store (internal_versions): skill, memory, kb-doc, kb-space, artifact
//   agent versions (agent_versions):    soul, config, personality
//
//   GET /api/history?kind=skill&owner=<owner>&name=<name>   → revisions
//   GET /api/history?kind=memory&id=<defId>                 → revisions
//   GET /api/history?kind=soul|config|personality&id=<defId>
//   &rev=<id>                                              → that revision's content
// ownerKey is derived server-side (skill: "<owner>/<name>", the rest: an id)
// so the caller never constructs the storage key. Version-backed kinds are
// admin-or-owner: souls and configs are the agent's internals, not public.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use crate::agent_defs::{AgentVersionRow, list_versions};
use crate::artifacts::{get_artifact, guarded};
use crate::body::utf16_len;
use crate::error::house_error;
use crate::google_oauth::query_pairs;
use crate::internal_history::{get_revision, list_history};
use crate::kb::{effective_doc_perms, get_doc, get_space, guarded_of_space};
use crate::kb_perms::{ITEM_ARTIFACT, ITEM_SPACE, can_read, list_editors};
use crate::personal_agent::{owns_agent, personality_of};
use crate::session::{SessionUser, require_user};
use crate::state::AppState;
use crate::yaml_string::stringify as stringify_yaml;

const SNAPSHOT_KINDS: &[&str] = &[
    "skill", "memory", "kb-doc", "kb-space", "artifact", "template",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VersionKind {
    Soul,
    Config,
    Personality,
}

impl VersionKind {
    fn parse(kind: &str) -> Option<Self> {
        match kind {
            "soul" => Some(Self::Soul),
            "config" => Some(Self::Config),
            "personality" => Some(Self::Personality),
            _ => None,
        }
    }
}

// versionContent — the materialized form each version kind is served in.
// `config` guards null with {} and personality falls back to "" (the ??
// arms in the TS ternary chain).
fn version_content(kind: VersionKind, v: &AgentVersionRow) -> String {
    match kind {
        VersionKind::Soul => v.soul.clone(),
        VersionKind::Config => {
            let empty = Value::Object(serde_json::Map::new());
            let config = if v.config.is_null() {
                &empty
            } else {
                &v.config
            };
            stringify_yaml(config)
        }
        VersionKind::Personality => personality_of(&v.soul).unwrap_or_default(),
    }
}

/// The version-kind revision summary — key order is the TS object literal's.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionRevision {
    id: String,
    created_by: Option<String>,
    created_at: String,
    size: usize,
    note: Option<String>,
    version: i32,
}

#[derive(serde::Serialize)]
struct RevisionsBody<R> {
    revisions: Vec<R>,
}

#[derive(serde::Serialize)]
struct ContentBody {
    content: String,
}

/// The live item's read model, applied to its history. Fail closed: every
/// miss and every error below reads as no (the TS try/catch swallows all).
async fn can_read_snapshot_history(
    state: &AppState,
    kind: &str,
    owner_key: &str,
    user: &SessionUser,
) -> bool {
    let who = user.email.as_deref().or(user.name.as_deref());
    match kind {
        "artifact" => match get_artifact(&state.pg, owner_key).await {
            Ok(Some(a)) => match list_editors(&state.pg, ITEM_ARTIFACT, &a.id).await {
                Ok(editors) => can_read(&guarded(&a), Some(&user.id), who, &editors),
                Err(_) => false,
            },
            _ => false,
        },
        "kb-doc" => match get_doc(&state.pg, owner_key).await {
            Ok(Some(d)) => match effective_doc_perms(&state.pg, &d).await {
                Ok(eff) => can_read(&eff.perms, Some(&user.id), who, &eff.grants),
                Err(_) => false,
            },
            _ => false,
        },
        "kb-space" => match get_space(&state.pg, owner_key).await {
            Ok(Some(sp)) => match list_editors(&state.pg, ITEM_SPACE, &sp.id).await {
                Ok(editors) => can_read(&guarded_of_space(&sp), Some(&user.id), who, &editors),
                Err(_) => false,
            },
            _ => false,
        },
        "memory" => {
            user.role == "admin" || owns_agent(&state.pg, &user.id, None, Some(owner_key)).await
        }
        "skill" => {
            // ownerKey is "<slug>/<name>" — shared skills + org agents are
            // admin-editable surfaces; a personal assistant's skills are its
            // owner's business.
            let slug = owner_key.split('/').next().unwrap_or("");
            user.role == "admin" || owns_agent(&state.pg, &user.id, Some(slug), None).await
        }
        "template" => user.role == "admin",
        _ => false,
    }
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let q = query_pairs(uri.query());
    let kind = q.get("kind").map(String::as_str);
    // `if (rev)` — an absent or EMPTY rev param lists, it doesn't select.
    let rev = q.get("rev").filter(|r| !r.is_empty()).map(String::as_str);

    if let Some(vkind) = kind.and_then(VersionKind::parse) {
        let id = q.get("id").filter(|i| !i.is_empty()).map(String::as_str);
        let Some(id) = id else {
            return house_error(StatusCode::BAD_REQUEST, "missing id");
        };
        if user.role != "admin" && !owns_agent(&state.pg, &user.id, None, Some(id)).await {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
        let versions = match list_versions(&state.pg, id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[history] versions read failed: {e}");
                return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
            }
        };
        if let Some(rev) = rev {
            return match versions.iter().find(|v| v.id == rev) {
                None => house_error(StatusCode::NOT_FOUND, "not found"),
                Some(v) => Json(ContentBody {
                    content: version_content(vkind, v),
                })
                .into_response(),
            };
        }
        let revisions: Vec<VersionRevision> = versions
            .iter()
            .take(50)
            .map(|v| VersionRevision {
                id: v.id.clone(),
                created_by: v.created_by.clone(),
                created_at: v.created_at.clone(),
                size: utf16_len(&version_content(vkind, v)),
                note: v.note.clone(),
                version: v.version,
            })
            .collect();
        return Json(RevisionsBody { revisions }).into_response();
    }

    if !kind.is_some_and(|k| SNAPSHOT_KINDS.contains(&k)) {
        return house_error(StatusCode::BAD_REQUEST, "bad kind");
    }
    // skill keys on "<owner>/<name>"; the rest key on an id. JS truthiness:
    // an empty owner/name/id is as good as absent.
    let owner_key = if kind == Some("skill") {
        match (
            q.get("owner").filter(|o| !o.is_empty()),
            q.get("name").filter(|n| !n.is_empty()),
        ) {
            (Some(owner), Some(name)) => Some(format!("{owner}/{name}")),
            _ => None,
        }
    } else {
        q.get("id").filter(|i| !i.is_empty()).cloned()
    };
    let Some(owner_key) = owner_key else {
        return house_error(StatusCode::BAD_REQUEST, "missing owner");
    };

    // History serves FULL content — it must honor the same read model as
    // the live item, or it's a bypass of the entire permission system.
    if !can_read_snapshot_history(&state, kind.unwrap(), &owner_key, &user).await {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }

    if let Some(rev) = rev {
        return match get_revision(&state.pg, kind.unwrap(), &owner_key, rev).await {
            Ok(Some(content)) => Json(ContentBody { content }).into_response(),
            Ok(None) => house_error(StatusCode::NOT_FOUND, "not found"),
            Err(e) => {
                tracing::error!("[history] revision read failed: {e}");
                house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            }
        };
    }
    match list_history(&state.pg, kind.unwrap(), &owner_key).await {
        Ok(revisions) => Json(RevisionsBody { revisions }).into_response(),
        Err(e) => {
            tracing::error!("[history] snapshot list failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal_history::Revision;

    fn version(soul: &str, config: Value) -> AgentVersionRow {
        AgentVersionRow {
            id: "v1".into(),
            agent_id: "a1".into(),
            version: 3,
            soul: soul.into(),
            config,
            note: None,
            created_by: None,
            created_at: "2026-08-29T10:00:00.000Z".into(),
        }
    }

    #[test]
    fn version_content_materializes_each_kind() {
        let v = version(
            "role\n<!-- talaria:personality -->\nwarm\n<!-- /talaria:personality -->\n",
            serde_json::json!({"model": "m"}),
        );
        assert_eq!(version_content(VersionKind::Soul, &v), v.soul);
        assert_eq!(version_content(VersionKind::Config, &v), "model: m\n");
        assert_eq!(version_content(VersionKind::Personality, &v), "warm");

        // null config → {} (the `?? {}` arm); no markers → "" (the `?? ""` arm)
        let null_cfg = version("s", Value::Null);
        assert_eq!(version_content(VersionKind::Config, &null_cfg), "{}\n");
        assert_eq!(version_content(VersionKind::Personality, &null_cfg), "");
    }

    #[test]
    fn sizes_are_utf16_like_the_ts_length() {
        // versionContent(...).length is JS UTF-16 length — an astral char
        // counts 2, the way the TS size field does.
        let v = version("a\u{1F600}b", Value::Null);
        assert_eq!(utf16_len(&version_content(VersionKind::Soul, &v)), 4);
    }

    #[test]
    fn revision_keys_serialize_in_ts_order() {
        let body = RevisionsBody {
            revisions: vec![VersionRevision {
                id: "v1".into(),
                created_by: None,
                created_at: "2026-08-29T10:00:00.000Z".into(),
                size: 5,
                note: None,
                version: 3,
            }],
        };
        let wire = serde_json::to_string(&body).unwrap();
        assert_eq!(
            wire,
            r#"{"revisions":[{"id":"v1","createdBy":null,"createdAt":"2026-08-29T10:00:00.000Z","size":5,"note":null,"version":3}]}"#
        );
    }

    #[test]
    fn snapshot_revision_keys_serialize_in_ts_order() {
        let body = RevisionsBody {
            revisions: vec![Revision {
                id: "r1".into(),
                created_by: Some("jon@x".into()),
                created_at: "2026-08-29T10:00:00.000Z".into(),
                size: 12,
            }],
        };
        let wire = serde_json::to_string(&body).unwrap();
        assert_eq!(
            wire,
            r#"{"revisions":[{"id":"r1","createdBy":"jon@x","createdAt":"2026-08-29T10:00:00.000Z","size":12}]}"#
        );
    }
}
