// Retrieval health + repair. The indexing pipeline is fire-and-forget by
// design (a write must never block on RAG), which means a dead Qdrant/TEI
// fails SILENTLY — the brains just stop filling. The defenses live across
// two files in TS: the health probe and the incremental sweep here
// (retrieval/backfill.ts), the durable repair runs next door in
// runs/defs/reindex.ts.
//
// WHAT HAS CROSSED: `rag_health`, the read plane (`backfill_status`) the
// admin rag route projects — the run row IS the truth now, not an
// app_settings blob beside it — and the 15-minute sweep
// (`sweepNewActivity`/`maybeRagSweep`), whose kick lives on the comms read
// (/api/channels) that crossed with the channels family.
// Port of ui/src/server/retrieval/backfill.ts.

use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;

use crate::retrieval::embed::{EmbedDeps, embed_one};
use crate::retrieval::index::IndexDoc;
use crate::retrieval::qdrant::QdrantDeps;
use crate::retrieval::sources::{
    EFFECTIVE_DOC_SELECT, KbDocSync, TicketSrc, index_activity, index_personal, index_ticket,
    kb_doc_of, sync_kb_doc,
};
use crate::runs::define::RunState;
use crate::runs::defs::reindex::BACKFILL_KIND;
use crate::runs::store::{KindRunView, latest_run_of_kind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RagHealth {
    pub qdrant: bool,
    pub embeddings: bool,
}

/// Probe both retrieval services (cheap, ~2s worst case). A probe that
/// errors reads as DOWN — the fetch sense of `r.ok`, never an exception.
pub async fn rag_health(qd: &QdrantDeps, ed: &EmbedDeps) -> RagHealth {
    // Two probes, one budget each, in parallel — the TS Promise.all. The
    // qdrant probe rides the same HttpFetch edge the client does so a test
    // scripts it like any other call; `ok` is the fetch sense (2xx).
    let qdrant_probe = (qd.fetch)(
        "GET",
        &format!("{}/collections", (qd.base)()),
        None,
        &[],
        2_500,
    );
    let embed_probe = embed_one(ed, "health probe");
    let (qdrant, embeddings) = tokio::join!(qdrant_probe, embed_probe);
    RagHealth {
        qdrant: matches!(qdrant, Ok((200..=299, _))),
        embeddings: embeddings.map(|v| !v.is_empty()).unwrap_or(false),
    }
}

// ── The backfill run's read shape ────────────────────────────────────────────

/// THE READ SHAPE, unchanged: `components/admin/retrieval.ts` declares exactly
/// these fields and polls while `state === 'running'`. The answer is a
/// projection of the `runs` row rather than a second copy of the truth.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillStatus {
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counts: Option<Value>,
}

/// Whatever the run has indexed so far. Read off the RESULT once it is done
/// and off the CHECKPOINT while it runs, which are the same numbers a step
/// apart — the driver persists the checkpoint before it takes the next step,
/// so the panel's tally can lag by one page and can never overstate. The
/// value passes through untouched: the keys are whatever the run wrote, and
/// both runtimes write the same bytes.
fn counts_of(run: &KindRunView) -> Option<Value> {
    run.result
        .get("counts")
        .or_else(|| run.checkpoint.get("counts"))
        .cloned()
}

/// Project a run row onto the four states the panel knows.
///
/// `queued` reads as RUNNING, and that is not a fudge: a queued run is one
/// the reclaim sweep will pick up within thirty seconds, or one a `retry`
/// has parked for a minute because Qdrant is down — in both cases the work
/// is in flight from the admin's point of view and the reason is in `phase`.
/// Showing it as idle would put the Start button back in front of somebody
/// whose backfill is about to resume, which is how you get two.
///
/// `cancelled` reads as DONE with the reason attached. The panel has no
/// fifth state and a stopped repair job is finished, not broken.
pub fn project_backfill_status(run: Option<&KindRunView>) -> BackfillStatus {
    let Some(run) = run else {
        return BackfillStatus {
            state: "idle",
            error: None,
            started_at: None,
            finished_at: None,
            counts: None,
        };
    };
    let (started_at, finished_at) = (run.started_at.clone(), run.finished_at.clone());
    let counts = counts_of(run);
    match run.state {
        RunState::Error => BackfillStatus {
            state: "error",
            error: Some(
                run.error
                    .clone()
                    .unwrap_or_else(|| "the backfill failed".into()),
            ),
            started_at,
            finished_at,
            counts,
        },
        RunState::Cancelled => BackfillStatus {
            state: "done",
            error: run.error.clone(),
            started_at,
            finished_at,
            counts,
        },
        RunState::Done => BackfillStatus {
            state: "done",
            error: None,
            started_at,
            finished_at,
            counts,
        },
        _ => BackfillStatus {
            state: "running",
            error: None,
            started_at,
            finished_at,
            counts,
        },
    }
}

/// A read the panel polls; a run row that cannot be read projects as idle
/// (TS's `.catch(() => null)`), never as a 500 that would take the whole
/// GET down with it.
pub async fn backfill_status(pg: &PgPool) -> BackfillStatus {
    project_backfill_status(
        latest_run_of_kind(pg, BACKFILL_KIND)
            .await
            .ok()
            .flatten()
            .as_ref(),
    )
}

// ── Incremental catch-up sweep ────────────────────────────────────────────────
// Event-driven indexing is the primary path; this 15-minute sweep re-indexes
// anything CREATED/UPDATED since the last high-water mark, so rows written
// while the services were down get picked up when they return. Content hashes
// make the overlap free. Crossed with the comms read that kicks it
// (/api/channels) — the TS header's "crosses with its callers" note.

/// The app_settings key holding the last sweep's high-water mark.
const SWEEP_KEY: &str = "rag_sweep_watermark";
const SWEEP_INTERVAL_MS: i64 = 15 * 60_000;

fn wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `Option<String>` → JSON string-or-null, preserving the key when absent
/// (the TS object literal keeps `null` valued keys, and the payload feeds the
/// content hash).
fn opt_json(v: &Option<String>) -> Value {
    match v {
        Some(s) => Value::String(s.clone()),
        None => Value::Null,
    }
}

pub async fn sweep_new_activity(pg: &PgPool, qd: &QdrantDeps, ed: &EmbedDeps) -> u32 {
    let health = rag_health(qd, ed).await;
    if !health.qdrant || !health.embeddings {
        return 0;
    }
    let watermark = crate::gateway::settings::get_setting(
        pg,
        SWEEP_KEY,
        serde_json::json!("1970-01-01T00:00:00.000Z"), // new Date(0).toISOString()
    )
    .await
    .as_str()
    .unwrap_or("1970-01-01T00:00:00.000Z")
    .to_string();
    let now = crate::agent_auth::epoch_ms_to_iso(wall_ms());
    let mut indexed: u32 = 0;

    // Effective visibility, exactly as the live save path resolves it — a doc
    // inheriting from a private space must never reach the org brain.
    // AssertSqlSafe: the interpolated fragment is this crate's EFFECTIVE_DOC_SELECT.
    let doc_rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{EFFECTIVE_DOC_SELECT} where d.updated_at > $1"
    )))
    .bind(&watermark)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    let docs: Vec<KbDocSync> = doc_rows.iter().map(kb_doc_of).collect();
    for d in &docs {
        let _ = sync_kb_doc(pg, qd, ed, d).await;
        indexed += 1;
    }

    let msgs: Vec<(String, String, String, String, String, String)> = sqlx::query_as(
        "select m.id::text, m.channel_id::text, m.author_type, m.author, m.content, c.name \
         from channel_messages m join channels c on c.id = m.channel_id \
         where m.status = 'complete' and m.content <> '' and c.kind <> 'dm' \
           and m.created_at > $1",
    )
    .bind(&watermark)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    for (id, channel_id, author_type, author, content, name) in &msgs {
        let who = if author_type == "agent" {
            crate::fleet::describe_agent(author).label
        } else {
            author.clone()
        };
        let _ = index_activity(
            pg,
            qd,
            ed,
            &IndexDoc {
                source_type: "channel".into(),
                source_id: id.clone(),
                title: Some(format!("#{name} · {who}")),
                text: content.clone(),
                payload: Some(serde_json::Map::from_iter([(
                    "channelId".to_string(),
                    Value::String(channel_id.clone()),
                )])),
                href: Some("/channels".into()),
            },
        )
        .await;
        indexed += 1;
    }

    let tasks: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        "select t.id::text, t.board_id::text, t.title, t.description \
         from tasks t where t.archived_at is null and t.updated_at > $1",
    )
    .bind(&watermark)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    for (id, board_id, title, description) in &tasks {
        let _ = index_ticket(
            pg,
            qd,
            ed,
            &TicketSrc {
                id,
                board_id,
                ticket_ref: None, // the sweep row selects no ref — "Ticket" default
                title,
                description: description.as_deref(),
            },
        )
        .await;
        indexed += 1;
    }

    #[allow(clippy::type_complexity)] // the artifact row's own columns, one each
    type ArtifactRow = (
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let arts: Vec<ArtifactRow> = sqlx::query_as(
        "select a.id::text, a.title, a.body, a.visibility, a.owner_user_id::text, a.rag_routing, \
                l.target_type, l.target_id::text \
         from artifacts a \
         left join artifact_links l on l.artifact_id = a.id and l.target_type in ('plan', 'research') \
         where a.kind = 'doc' and a.body <> '' and a.updated_at > $1 and l.target_type is not null",
    )
    .bind(&watermark)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    for (id, title, body, visibility, owner_id, rag_routing, target_type, target_id) in &arts {
        if rag_routing
            .as_deref()
            .is_some_and(|r| !r.is_empty() && r != "auto")
        {
            // Routed artifact: re-place it by its routing rule, not the default.
            if let Ok(Some(full)) = crate::artifacts::get_artifact(pg, id).await {
                crate::retrieval::artifact_routing::apply_artifact_routing(pg, qd, ed, &full).await;
            }
            indexed += 1;
            continue;
        }
        let is_plan = target_type.as_deref() == Some("plan");
        // The TS object literals keep null valued keys (`planId: a.targetId`
        // with targetId null serializes `"planId":null`) — and the payload
        // feeds the content hash, so presence must match or a TS-written point
        // churns on its first Rust re-index.
        let mut payload = serde_json::Map::new();
        if is_plan {
            payload.insert("planId".into(), opt_json(target_id));
            payload.insert("planOwnerId".into(), opt_json(owner_id));
        } else if owner_id.is_some() {
            payload.insert("runId".into(), opt_json(target_id));
        } else {
            payload.insert("runId".into(), opt_json(target_id));
            payload.insert("orgWide".into(), Value::Bool(true));
        }
        let doc = IndexDoc {
            source_type: if is_plan { "plan-doc" } else { "research" }.into(),
            source_id: id.clone(),
            title: Some(title.clone()),
            text: format!("{title}\n\n{body}"),
            payload: Some(payload),
            href: Some(if is_plan {
                "/artifacts".into()
            } else {
                format!("/research/{}", target_id.clone().unwrap_or_default())
            }),
        };
        if !is_plan && visibility == "private" {
            if let Some(owner) = owner_id {
                index_personal(pg, qd, ed, owner, &doc).await;
            }
        } else {
            let _ = index_activity(pg, qd, ed, &doc).await;
        }
        indexed += 1;
    }

    let _ = crate::gateway::settings::set_setting(pg, SWEEP_KEY, &Value::String(now)).await;
    indexed
}

/// Opportunistic scheduling: any comms/search read may kick a sweep, at most
/// every 15 minutes, never blocking. Process-local, like the TS module boolean.
static LAST_SWEEP_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

pub fn maybe_rag_sweep(state: crate::state::AppState) {
    let now = wall_ms();
    let last = LAST_SWEEP_MS.load(std::sync::atomic::Ordering::Relaxed);
    if now - last < SWEEP_INTERVAL_MS {
        return;
    }
    LAST_SWEEP_MS.store(now, std::sync::atomic::Ordering::Relaxed);
    tokio::spawn(async move {
        let qd = crate::retrieval::qdrant::real_deps();
        let ed = crate::retrieval::embed::real_deps();
        let _ = sweep_new_activity(&state.pg, &qd, &ed).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::define::RunState;
    use serde_json::json;

    fn view(state: RunState) -> KindRunView {
        KindRunView {
            id: "r1".into(),
            state,
            phase: String::new(),
            input: json!({}),
            checkpoint: Value::Null,
            result: Value::Null,
            error: None,
            started_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn the_backfill_projection_reads_the_four_states() {
        // No run at all: idle, and nothing else on the wire.
        let idle = project_backfill_status(None);
        assert_eq!(idle.state, "idle");
        assert_eq!(
            serde_json::to_value(&idle).unwrap(),
            json!({"state": "idle"})
        );

        // Running: the tally comes off the checkpoint while it runs.
        let mut running = view(RunState::Running);
        running.checkpoint = json!({"source": "kb-docs", "counts": {"kbDocs": 140}});
        assert_eq!(
            serde_json::to_value(project_backfill_status(Some(&running))).unwrap(),
            json!({"state": "running", "counts": {"kbDocs": 140}})
        );
        // Queued is running too — the sweep picks it up within the minute.
        assert_eq!(
            project_backfill_status(Some(&view(RunState::Queued))).state,
            "running"
        );

        // Done: the RESULT's tally wins over a checkpoint left behind.
        let mut done = view(RunState::Done);
        done.checkpoint = json!({"counts": {"kbDocs": 140}});
        done.result = json!({"counts": {"kbDocs": 140, "tickets": 3}});
        assert_eq!(
            serde_json::to_value(project_backfill_status(Some(&done))).unwrap(),
            json!({"state": "done", "counts": {"kbDocs": 140, "tickets": 3}})
        );

        // Error: the reason, with the default sentence when the row has none.
        let mut failed = view(RunState::Error);
        failed.error = Some("qdrant went away".into());
        assert_eq!(
            serde_json::to_value(project_backfill_status(Some(&failed))).unwrap(),
            json!({"state": "error", "error": "qdrant went away"})
        );
        assert_eq!(
            serde_json::to_value(project_backfill_status(Some(&view(RunState::Error)))).unwrap(),
            json!({"state": "error", "error": "the backfill failed"})
        );

        // Cancelled reads as done with the reason attached.
        let mut cancelled = view(RunState::Cancelled);
        cancelled.error = Some("admin stopped it".into());
        assert_eq!(
            serde_json::to_value(project_backfill_status(Some(&cancelled))).unwrap(),
            json!({"state": "done", "error": "admin stopped it"})
        );
    }
}
