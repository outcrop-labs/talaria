// /api/artifacts. The artifact LIST
// (what the Files browser opens on) and CREATE. Read is gated exactly like
// the KB list beside it: org/public visible to all, private only to owner,
// grants — and, for a personal assistant, its owner's own reach (the single-
// artifact GET has always had that arm; the LIST now mirrors it). Creation
// differs by caller: a PERSONAL assistant's output belongs to its owner and
// starts private-to-them; a general agent's output is workspace material and
// starts org-visible — stamped to the human the agent answers to (the
// attribution ladder), so a person governs it instead of an allow-list.

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::{AgentCaller, AgentSubject, agent_caller, iso_to_epoch_ms};
use talaria_api_facades::fleet::describe_agent;
use talaria_api_facades::kb::perms::{
    EditorGrant, ITEM_ARTIFACT, can_read, granted_item_ids, set_editors,
};
use talaria_artifacts::agent_access::agent_readable;
use talaria_artifacts::{
    AGENT_CATEGORIES, Artifact, ArtifactHead, ArtifactMeta, SaveArtifactPatch,
    agent_category_folder, create_artifact, guarded, list_artifact_meta, list_artifacts,
    list_artifacts_since, named_root_folder, save_artifact,
};
use talaria_audit::{AuditEntry, log_audit, spawn_audit};
use talaria_body::{
    optional_enum_member, optional_max_string_member, parse, too_big_msg, zod_type_name,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{SessionUser, actor_of, require_perm, require_user, who_of};
use talaria_state::AppState;

struct Body {
    kind: Option<String>,
    title: Option<String>,
    body: Option<String>,
    folder: Option<String>,
}

fn parse_body(obj: &serde_json::Map<String, Value>) -> Result<Body, String> {
    let parsed = Body {
        kind: optional_enum_member(obj, "kind", &["doc", "sheet", "microsite", "file"])?,
        title: optional_max_string_member(obj, "title", 200)?,
        body: optional_max_string_member(obj, "body", 2_000_000)?,
        // folder: trimmed, max 120, optional — the only trimmed-OPTIONAL
        // member on this surface, so it reads inline rather than growing a
        // body.rs helper for one caller.
        folder: match obj.get("folder") {
            None => None,
            Some(v) => {
                let s = v
                    .as_str()
                    .ok_or_else(|| {
                        format!(
                            "Invalid input: expected string, received {}",
                            zod_type_name(v)
                        )
                    })?
                    .trim()
                    .to_string();
                if talaria_body::utf16_len(&s) > 120 {
                    return Err(too_big_msg(120));
                }
                Some(s)
            }
        },
    };
    // `visibility` is still VALIDATED (a bad enum is the same 400 it always
    // was, in the same order) and then dropped: no creator on this route gets
    // to choose it. A personal assistant's output is private to its owner, an
    // org agent's is org — `public` is a human power behind
    // `artifacts.publish`, granted on the PUT — and a person's create starts
    // private under the engine's own default, published later from the Files
    // browser.
    optional_enum_member(obj, "visibility", &["private", "org", "public"])?;
    Ok(parsed)
}

/// The list's two optional narrowings, read from the query string. Absent
/// and empty are the same query, and neither changes what a viewer may see:
///   since=<ISO instant>  only artifacts updated at or after it
///   fields=heads         every column but the body
/// The web Inbox asks for both (its Files cards are today's documents, by
/// title); the Files browser and KB attachments ask for neither.
#[derive(Debug, Default, PartialEq)]
struct ListQuery {
    since_ms: Option<i64>,
    heads: bool,
}

fn list_query(q: &HashMap<String, String>) -> Result<ListQuery, String> {
    let since_ms = match q.get("since").map(String::as_str) {
        None | Some("") => None,
        Some(s) => Some(iso_to_epoch_ms(s).ok_or_else(|| {
            format!("since must be an ISO time like 2026-11-20T08:00:00.000Z, not {s:?}")
        })?),
    };
    let heads = match q.get("fields").map(String::as_str) {
        None | Some("") => false,
        Some("heads") => true,
        Some(other) => return Err(format!("fields must be heads, not {other:?}")),
    };
    Ok(ListQuery { since_ms, heads })
}

/// Who is listing, and what the read filter needs of them.
enum Viewer {
    /// An agent: the batch agent read rule (`agent_readable`).
    Agent(AgentCaller),
    /// A person: their grants, then the KB read rule.
    Person {
        id: String,
        who: Option<String>,
        granted: HashSet<String>,
    },
}

/// The rows `viewer` may read, in their order. The LIST must answer the same
/// question the single-artifact GET answers: for an agent, org/public
/// artifacts, ones granted to it or a team it is on, and — for a personal
/// assistant — its owner's own private ones (talaria-artifacts' batch rule,
/// not a copy of it); for a person, a grant or the read rule. Whole rows and
/// heads go through the same filter, by their head.
async fn readable<T>(
    pg: &sqlx::PgPool,
    viewer: &Viewer,
    rows: Vec<T>,
    head: impl Fn(&T) -> ArtifactHead,
) -> Result<Vec<T>, Response> {
    let heads: Vec<ArtifactHead> = rows.iter().map(head).collect();
    let keep: Vec<bool> = match viewer {
        Viewer::Agent(caller) => {
            let ok = agent_readable(pg, caller, &heads)
                .await
                .map_err(|e| internal("[artifacts] agent read check failed", e))?;
            heads.iter().map(|h| ok.contains(&h.id)).collect()
        }
        Viewer::Person { id, who, granted } => heads
            .iter()
            .map(|h| {
                granted.contains(&h.id) || can_read(&h.guarded, Some(id), who.as_deref(), &[], &[])
            })
            .collect(),
    };
    Ok(rows
        .into_iter()
        .zip(keep)
        .filter_map(|(r, k)| k.then_some(r))
        .collect())
}

fn whole_head(a: &Artifact) -> ArtifactHead {
    ArtifactHead {
        id: a.id.clone(),
        kind: a.kind.clone(),
        title: a.title.clone(),
        guarded: guarded(a),
    }
}

/// A signed-in person as the read filter sees them: their grants, and the
/// author string older rows were stamped with.
async fn person_viewer(pg: &sqlx::PgPool, user: &SessionUser) -> Result<Viewer, Response> {
    let granted = granted_item_ids(pg, ITEM_ARTIFACT, &user.id)
        .await
        .map_err(|e| internal("[artifacts] grants read failed", e))?;
    Ok(Viewer::Person {
        id: user.id.clone(),
        who: who_of(user),
        granted,
    })
}

/// The list `viewer` may read, narrowed by `query` — the answer's `artifacts`
/// once the caller is known.
async fn list_for(
    pg: &sqlx::PgPool,
    viewer: &Viewer,
    query: &ListQuery,
) -> Result<Value, Response> {
    let failed = |e: sqlx::Error| internal("[artifacts] list failed", e);
    if query.heads {
        let rows = list_artifact_meta(pg, query.since_ms)
            .await
            .map_err(failed)?;
        return Ok(json!(readable(pg, viewer, rows, ArtifactMeta::head).await?));
    }
    let rows = match query.since_ms {
        Some(ms) => list_artifacts_since(pg, ms).await,
        None => list_artifacts(pg).await,
    }
    .map_err(failed)?;
    Ok(json!(readable(pg, viewer, rows, whole_head).await?))
}

// doc: The artifacts the caller may read, newest first. Two optional
// doc: narrowings, neither of which widens what a caller sees:
// doc: `?since=<ISO instant>` keeps those updated at or after it, and
// doc: `?fields=heads` drops the body. A malformed value is a 400.
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, Response> {
    let viewer = match agent_caller(&state.pg, &headers).await? {
        Some(caller) => Viewer::Agent(caller),
        None => person_viewer(&state.pg, &require_user(&state, &headers).await?).await?,
    };
    let query = match list_query(&q) {
        Ok(query) => query,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let artifacts = list_for(&state.pg, &viewer, &query).await?;
    Ok(Json(json!({ "artifacts": artifacts })).into_response())
}

/// THE NEW FILE'S CHIP, landed by the route that made it — the edit route's
/// rule (`artifacts_id_edit::land_document_chip`) for a create. The tool-events
/// door lands the same chip from the tool's answer, but that answer arrives
/// cut to 4000 chars by the agent's plugin, and a create answers with the
/// whole body, so a document of any length lost its id there and never joined
/// the session's files — never opened in the Work pane. Same chip id, so
/// whichever lands second merges onto the first. Detached: the create already
/// happened.
///
/// ONLY ON A LIVE TURN (`surface_on_live_turn`). A create with no streaming
/// reply — a cron run, a background job — lands nothing, rather than opening
/// the file in the pane of whoever the agent last spoke to.
fn land_created_chip(state: &AppState, agent: &str, artifact: &Artifact) {
    let chip = talaria_chips::document_chip(&artifact.id, Some(&artifact.title), None);
    let (state, agent) = (state.clone(), agent.to_string());
    tokio::spawn(async move {
        let redis = state.redis().await.ok();
        if let Err(e) =
            talaria_chips::surface_on_live_turn(&state.pg, redis, &agent, vec![chip]).await
        {
            tracing::warn!("[artifacts] create chip surface failed for {agent}: {e}");
        }
    });
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let body = match parse_body(obj) {
        Ok(b) => b,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    let caller = agent_caller(&state.pg, &headers).await?;
    if let Some(caller) = caller {
        let name = caller.model.clone();
        // WHO the agent works for decides two things, deliberately two
        // different answers. A PERSONAL assistant's output stays private to
        // its owner (they can share it; the assistant cannot make it
        // org-wide) — pa_owner drives that visibility branch below. The
        // owner STAMP is the attribution ladder: a PA's owner, or the human
        // an org agent is answering — a general org agent's output stays
        // org-visible, but now belongs to a person who can govern it instead
        // of an allow-list. Both asks need the CALLER: attaching output to a
        // human's account is owner-proxying, proven identity only.
        let pa_owner = match talaria_users::assistant_owner_for(
            &state.pg,
            &AgentSubject::Caller(caller.clone()),
        )
        .await
        {
            Ok(o) => o,
            Err(e) => return Ok(internal("[artifacts] owner lookup failed", e)),
        };
        let responsible = match talaria_attribution::responsible_user_for(
            &state.pg,
            state.redis().await.ok(),
            &AgentSubject::Caller(caller),
        )
        .await
        {
            Ok(o) => o,
            Err(e) => return Ok(internal("[artifacts] responsible-user lookup failed", e)),
        };
        let folder_id = match body.folder.as_deref() {
            Some(f) if !f.is_empty() => named_root_folder(&state.pg, f, &name).await,
            _ => None,
        };
        let cabinet = agent_category_folder(
            &state.pg,
            &describe_agent(&name).label,
            AGENT_CATEGORIES[0], // "Documents"
            &name,
        )
        .await;
        let artifact = match create_artifact(
            &state.pg,
            body.kind.as_deref(),
            body.title.as_deref(),
            &name,
            responsible.as_deref(),
            // Named folder (find-or-create) when the agent files deliberately;
            // its own cabinet otherwise.
            folder_id.or(cabinet).as_deref(),
        )
        .await
        {
            Ok(a) => a,
            Err(e) => return Ok(internal("[artifacts] create failed", e)),
        };
        if let Err(e) = set_editors(
            &state.pg,
            ITEM_ARTIFACT,
            &artifact.id,
            &[EditorGrant {
                principal_type: "agent".into(),
                principal_id: name.clone(),
                role: "editor".into(),
            }],
        )
        .await
        {
            return Ok(internal("[artifacts] editor grant failed", e));
        }
        let updated = match save_artifact(
            &state.pg,
            &artifact.id,
            if pa_owner.is_some() {
                SaveArtifactPatch {
                    body: body.body.as_deref(),
                    visibility: Some("private"),
                    edit_policy: Some("owner"),
                    ..Default::default()
                }
            } else {
                // ALWAYS `org`, whatever the body asked for. Publishing to
                // the unauthenticated web is a human power behind
                // `artifacts.publish`; the PUT strips `visibility` from an
                // agent key for exactly that reason, and a create that
                // honoured `public` was the one door left open. A person who
                // wants it public publishes it from the Files browser.
                SaveArtifactPatch {
                    body: body.body.as_deref(),
                    visibility: Some("org"),
                    edit_policy: Some("org"),
                    ..Default::default()
                }
            },
            &name,
        )
        .await
        {
            Ok(u) => u,
            Err(e) => return Ok(internal("[artifacts] save failed", e)),
        };
        // The agent plane audits its creates like the human plane does — a
        // document an agent wrote into the workspace is a governance event
        // someone may need to trace back.
        spawn_audit(
            &state.pg,
            format!("agent:{name}"),
            "artifact.create",
            "artifact",
            Some(artifact.id.clone()),
            Some(json!({ "title": artifact.title, "kind": artifact.kind })),
        );
        let created = updated.unwrap_or(artifact);
        land_created_chip(&state, &name, &created);
        return Ok(Json(json!({ "artifact": created })).into_response());
    }

    let user = require_perm(&state, &headers, "artifacts.create").await?;
    let actor = who_of(&user).unwrap_or_else(|| "user".into());
    let artifact = match create_artifact(
        &state.pg,
        body.kind.as_deref(),
        body.title.as_deref(),
        &actor,
        Some(&user.id),
        None,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => return Ok(internal("[artifacts] create failed", e)),
    };
    let updated = if body.body.is_some() {
        match save_artifact(
            &state.pg,
            &artifact.id,
            SaveArtifactPatch {
                body: body.body.as_deref(),
                ..Default::default()
            },
            &actor,
        )
        .await
        {
            Ok(u) => u,
            Err(e) => return Ok(internal("[artifacts] save failed", e)),
        }
    } else {
        None
    };
    let (pg, audit_actor, target_id, target_label) = (
        state.pg.clone(),
        actor_of(&user),
        artifact.id.clone(),
        artifact.title.clone(),
    );
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &audit_actor,
                action: "artifact.create",
                target_type: "artifact",
                target_id: Some(&target_id),
                target_label: Some(&target_label),
                before: None,
                after: None,
            },
        )
        .await;
    });
    Ok(Json(json!({ "artifact": updated.unwrap_or(artifact) })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn no_narrowing_is_the_whole_list() {
        assert_eq!(list_query(&q(&[])), Ok(ListQuery::default()));
        // Absent and empty are the same query.
        assert_eq!(
            list_query(&q(&[("since", ""), ("fields", "")])),
            Ok(ListQuery::default())
        );
        // A parameter the list does not know is ignored, as it always was.
        assert_eq!(list_query(&q(&[("folder", "x")])), Ok(ListQuery::default()));
    }

    #[test]
    fn since_takes_the_instant_a_browser_writes() {
        // `new Date(y, m, d).toISOString()` — the reader's local midnight in UTC.
        assert_eq!(
            list_query(&q(&[
                ("since", "2026-11-20T08:00:00.000Z"),
                ("fields", "heads")
            ])),
            Ok(ListQuery {
                since_ms: Some(1_795_161_600_000),
                heads: true,
            })
        );
        assert_eq!(
            list_query(&q(&[("since", "2026-11-20T08:00:00Z")])).map(|l| l.since_ms),
            Ok(Some(1_795_161_600_000))
        );
    }

    #[test]
    fn a_bad_narrowing_is_refused_not_ignored() {
        for bad in [
            "yesterday",
            "2026-11-20",
            "1795680000000",
            "2026-13-01T00:00:00Z",
        ] {
            let err = list_query(&q(&[("since", bad)])).unwrap_err();
            assert!(err.starts_with("since must be an ISO time"), "{bad}: {err}");
        }
        let err = list_query(&q(&[("fields", "body")])).unwrap_err();
        assert_eq!(err, r#"fields must be heads, not "body""#);
    }
}

#[cfg(test)]
#[path = "artifacts_live.rs"]
mod artifacts_live;
