// Retrieval health + repair. The indexing pipeline is fire-and-forget by
// design (a write must never block on RAG), which means a dead Qdrant/TEI
// fails SILENTLY — the brains just stop filling. The defenses live across
// two files in TS: the health probe and the incremental sweep here
// (retrieval/backfill.ts), the durable repair runs next door in
// runs/defs/reindex.ts.
//
// WHAT HAS CROSSED: `rag_health`, and the read plane (`backfill_status`) the
// admin rag route projects — the run row IS the truth now, not an
// app_settings blob beside it. What STAYS TS is the 15-minute sweep
// (`sweepNewActivity`/`maybeRagSweep`) — it crosses with its callers (the
// comms/search reads) rather than landing as unreachable code.
// Port of ui/src/server/retrieval/backfill.ts.

use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;

use crate::retrieval::embed::{EmbedDeps, embed_one};
use crate::retrieval::qdrant::QdrantDeps;
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
