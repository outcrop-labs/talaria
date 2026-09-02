// Guided reindex — the repair path for the two ways the retrieval plane goes
// stale: the embedding model changed (TALARIA_EMBED_MODEL swap → new vector
// dimension → every index/search call fails against the old collections), or
// a collection still has the legacy dense-only (v1) schema and is missing
// hybrid keyword recall.
//
// Decisions read the LIVE Qdrant collection config, never the registry
// columns (those went stale once already: rows stamped 1024 while the
// collections were 384). The rebuild itself is a durable run — the steps,
// the checkpoint and the re-entry rules live in runs/defs/reindex.rs; what
// lives here is the live upgrade status (the 60s cache /alerts polls), the
// reindex run's READ SHAPE the admin panel consumes, and the cache drop the
// run calls after every collection it rebuilds.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use sqlx::Row;

use crate::retrieval::embed::{EmbedDeps, EmbedInfo, embed_info};
use crate::retrieval::qdrant::{QdrantDeps, collection_info};
use crate::runs::define::RunState;
use crate::runs::defs::reindex::REINDEX_KIND;
use crate::runs::store::{KindRunView, latest_run_of_kind};

const CACHE_MS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionStatus {
    pub id: String,
    pub name: String,
    pub qdrant_name: String,
    pub points_count: i64,
    pub dense_dim: Option<i64>,
    pub hybrid: bool,
    /// This collection's dense dim doesn't match the live embedding model.
    pub dim_mismatch: bool,
    pub missing: bool,
}

/// The wire view of the embed service — `{modelId, dim}` camelCase, the
/// shape the panel consumes. (EmbedInfo itself is the internal shape;
/// this is the one that rides a response body.)
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbedInfoPublic {
    pub model_id: String,
    pub dim: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalUpgradeStatus {
    pub embed: Option<EmbedInfoPublic>,
    pub collections: Vec<CollectionStatus>,
    /// Any collection whose vectors no longer match the live model — indexing
    /// and search against it are failing right now.
    pub dim_mismatch: bool,
    /// Any legacy dense-only collection — hybrid keyword recall available.
    pub legacy_schema: bool,
    pub needs_reindex: bool,
}

// ── The 60s status cache ─────────────────────────────────────────────────────
//
// Process-local. The reindex run drops it through
// `invalidate_upgrade_status` after every collection it rebuilds — a cached
// "needs reindex" surviving the rebuild that fixed it is an alarm that trains
// people to ignore alarms.

struct CacheEntry {
    at: Instant,
    value: RetrievalUpgradeStatus,
}

fn cache_cell() -> &'static Mutex<Option<CacheEntry>> {
    static CELL: OnceLock<Mutex<Option<CacheEntry>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// `now` is a parameter so the window is testable without sleeping.
fn cached_status(now: Instant) -> Option<RetrievalUpgradeStatus> {
    cache_cell()
        .lock()
        .unwrap()
        .as_ref()
        .filter(|e| now.duration_since(e.at) < Duration::from_millis(CACHE_MS))
        .map(|e| e.value.clone())
}

fn store_status(at: Instant, value: RetrievalUpgradeStatus) {
    *cache_cell().lock().unwrap() = Some(CacheEntry { at, value });
}

/// Drop the 60s status cache. Called by the reindex run after every
/// collection it rebuilds.
pub fn invalidate_upgrade_status() {
    *cache_cell().lock().unwrap() = None;
}

/// Live upgrade status (60s cache — alerts poll this). Any failure reading
/// the systems of record is the CALLER's to fold (the admin route shows null;
/// a cache entry is stored only on success, so a failure never pins a stale
/// answer in place).
pub async fn retrieval_upgrade_status(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
) -> Result<RetrievalUpgradeStatus, sqlx::Error> {
    if let Some(v) = cached_status(Instant::now()) {
        return Ok(v);
    }
    let value = compute_upgrade_status(pg, qd, ed).await?;
    store_status(Instant::now(), value.clone());
    Ok(value)
}

async fn compute_upgrade_status(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
) -> Result<RetrievalUpgradeStatus, sqlx::Error> {
    let embed = embed_info(ed).await;
    let rows = sqlx::query(
        "select id::text, name, qdrant_name from rag_collections \
         order by auto desc, name asc",
    )
    .fetch_all(pg)
    .await?;
    let mut collections = Vec::with_capacity(rows.len());
    for r in rows {
        let qdrant_name: String = r.try_get("qdrant_name")?;
        let info = collection_info(qd, &qdrant_name).await;
        collections.push(CollectionStatus {
            id: r.try_get("id")?,
            name: r.try_get("name")?,
            qdrant_name,
            points_count: info.as_ref().map(|i| i.points_count).unwrap_or(0),
            dense_dim: info.as_ref().and_then(|i| i.dense_dim),
            hybrid: info.as_ref().map(|i| i.hybrid).unwrap_or(false),
            dim_mismatch: match (&embed, &info) {
                // A dense dim of 0 counts as absent — it never flags a
                // mismatch.
                (Some(e), Some(i)) => i.dense_dim.is_some_and(|d| d != 0 && d != e.dim as i64),
                _ => false,
            },
            missing: info.is_none(),
        });
    }
    Ok(fold_flags(embed, collections))
}

/// The flag fold, pure so the booleans' exact logic is pinnable without a
/// live Qdrant: dimMismatch and legacySchema name the two staleness modes,
/// and needsReindex is their disjunction plus a missing collection — but
/// only when the embed service is up (no embed = nothing actionable; the
/// existing rag-down alert covers it).
fn fold_flags(
    embed: Option<EmbedInfo>,
    collections: Vec<CollectionStatus>,
) -> RetrievalUpgradeStatus {
    let dim_mismatch = collections.iter().any(|c| c.dim_mismatch);
    let legacy_schema = collections.iter().any(|c| !c.missing && !c.hybrid);
    let any_missing = collections.iter().any(|c| c.missing);
    // No embed service = nothing actionable; the existing rag-down alert
    // covers the outage itself. Read before the map consumes `embed`.
    let needs_reindex = embed.is_some() && (dim_mismatch || legacy_schema || any_missing);
    RetrievalUpgradeStatus {
        embed: embed.map(|e| EmbedInfoPublic {
            model_id: e.model_id,
            dim: e.dim,
        }),
        collections,
        dim_mismatch,
        legacy_schema,
        needs_reindex,
    }
}

// ── The reindex run's read shape ─────────────────────────────────────────────

/// THE READ SHAPE: the admin panel declares exactly these fields and polls
/// while `state === 'running'`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReindexStatus {
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

/// Project the run row onto the four states the panel knows. Same rules as
/// the backfill's projection next door and for the same reasons: `queued` is
/// RUNNING (a reclaim or a `retry` will move it within the minute, and the
/// reason is in `phase`), `cancelled` is DONE with the reason attached.
pub fn project_reindex_status(run: Option<&KindRunView>) -> ReindexStatus {
    let Some(run) = run else {
        return ReindexStatus {
            state: "idle",
            error: None,
            phase: None,
            started_at: None,
            finished_at: None,
        };
    };
    let (started_at, finished_at) = (run.started_at.clone(), run.finished_at.clone());
    match run.state {
        RunState::Error => ReindexStatus {
            state: "error",
            error: Some(
                run.error
                    .clone()
                    .unwrap_or_else(|| "the rebuild failed".into()),
            ),
            phase: None,
            started_at,
            finished_at,
        },
        RunState::Cancelled => ReindexStatus {
            state: "done",
            error: run.error.clone(),
            phase: None,
            started_at,
            finished_at,
        },
        RunState::Done => ReindexStatus {
            state: "done",
            error: None,
            phase: None,
            started_at,
            finished_at,
        },
        // The two words the panel prints come straight off the checkpoint,
        // which is the run's own statement of which half it is in. Before the
        // first checkpoint there is no half yet, and 'rebuilding' is what it
        // is about to do.
        _ => ReindexStatus {
            state: "running",
            error: None,
            phase: Some(
                run.checkpoint
                    .get("phase")
                    .and_then(Value::as_str)
                    .unwrap_or("rebuilding")
                    .to_string(),
            ),
            started_at,
            finished_at,
        },
    }
}

pub async fn reindex_status(pg: &PgPool) -> ReindexStatus {
    project_reindex_status(
        latest_run_of_kind(pg, REINDEX_KIND)
            .await
            .ok()
            .flatten()
            .as_ref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn the_reindex_projection_reads_the_four_states() {
        // No run at all: idle, and nothing else on the wire.
        let idle = project_reindex_status(None);
        assert_eq!(idle.state, "idle");
        assert_eq!(
            serde_json::to_value(&idle).unwrap(),
            json!({"state": "idle"})
        );

        // Running: the phase is the checkpoint's own statement of which half
        // it is in, and a missing checkpoint still says 'rebuilding'.
        let mut running = view(RunState::Running);
        running.checkpoint = json!({"phase": "backfilling"});
        let running_json = serde_json::to_value(project_reindex_status(Some(&running))).unwrap();
        assert_eq!(
            running_json,
            json!({"state": "running", "phase": "backfilling"})
        );
        assert_eq!(
            serde_json::to_value(project_reindex_status(Some(&view(RunState::Queued)))).unwrap(),
            json!({"state": "running", "phase": "rebuilding"})
        );

        // Error: the reason, with the default sentence when the row has none.
        let mut failed = view(RunState::Error);
        failed.error = Some("qdrant vanished mid-rebuild".into());
        assert_eq!(
            serde_json::to_value(project_reindex_status(Some(&failed))).unwrap(),
            json!({"state": "error", "error": "qdrant vanished mid-rebuild"})
        );
        assert_eq!(
            serde_json::to_value(project_reindex_status(Some(&view(RunState::Error)))).unwrap(),
            json!({"state": "error", "error": "the rebuild failed"})
        );

        // Cancelled reads as done with the reason attached; done is done.
        let mut cancelled = view(RunState::Cancelled);
        cancelled.error = Some("admin stopped it".into());
        assert_eq!(
            serde_json::to_value(project_reindex_status(Some(&cancelled))).unwrap(),
            json!({"state": "done", "error": "admin stopped it"})
        );
        assert_eq!(
            serde_json::to_value(project_reindex_status(Some(&view(RunState::Done)))).unwrap(),
            json!({"state": "done"})
        );
    }

    #[test]
    fn the_projection_carries_the_timestamps() {
        let mut run = view(RunState::Running);
        run.started_at = Some("2026-08-30T00:00:00.000Z".into());
        run.finished_at = Some("2026-08-30T00:01:00.000Z".into());
        assert_eq!(
            serde_json::to_value(project_reindex_status(Some(&run))).unwrap(),
            json!({
                "state": "running", "phase": "rebuilding",
                "startedAt": "2026-08-30T00:00:00.000Z",
                "finishedAt": "2026-08-30T00:01:00.000Z",
            })
        );
    }

    fn col(dim_mismatch: bool, hybrid: bool, missing: bool) -> CollectionStatus {
        CollectionStatus {
            id: "c".into(),
            name: "Brain".into(),
            qdrant_name: "talaria_brain".into(),
            points_count: 10,
            dense_dim: if missing { None } else { Some(1536) },
            hybrid,
            dim_mismatch,
            missing,
        }
    }

    #[test]
    fn the_upgrade_flags_fold_ts_verbatim() {
        // The two staleness modes plus a missing collection, with the embed
        // service up: everything is flagged, and needsReindex is the
        // disjunction.
        let flagged = fold_flags(
            Some(EmbedInfo {
                model_id: "bge-m3".into(),
                dim: 1024,
            }),
            vec![
                col(true, true, false),
                col(false, false, false),
                col(false, true, true),
            ],
        );
        assert!(flagged.dim_mismatch);
        assert!(flagged.legacy_schema);
        assert!(flagged.needs_reindex);

        // Healthy everywhere: nothing to do.
        let healthy = fold_flags(
            Some(EmbedInfo {
                model_id: "bge-m3".into(),
                dim: 1536,
            }),
            vec![col(false, true, false)],
        );
        assert!(!healthy.dim_mismatch);
        assert!(!healthy.legacy_schema);
        assert!(!healthy.needs_reindex);

        // No embed service = nothing actionable, whatever the collections
        // look like — the rag-down alert covers the outage itself.
        let dark = fold_flags(None, vec![col(true, false, false)]);
        assert!(dark.dim_mismatch);
        assert!(dark.legacy_schema);
        assert!(!dark.needs_reindex);
        assert!(dark.embed.is_none());
    }

    #[test]
    fn the_upgrade_status_serializes_the_ts_wire_shape() {
        let s = fold_flags(
            Some(EmbedInfo {
                model_id: "bge-m3".into(),
                dim: 1024,
            }),
            vec![col(false, true, false)],
        );
        let j = serde_json::to_value(&s).unwrap();
        // Field order is the struct's declaration order, camelCase — the
        // bytes the panel was built against.
        assert_eq!(
            j,
            json!({
                "embed": {"modelId": "bge-m3", "dim": 1024},
                "collections": [{
                    "id": "c", "name": "Brain", "qdrantName": "talaria_brain",
                    "pointsCount": 10, "denseDim": 1536, "hybrid": true,
                    "dimMismatch": false, "missing": false,
                }],
                "dimMismatch": false,
                "legacySchema": false,
                "needsReindex": false,
            })
        );
    }

    #[test]
    fn the_status_cache_serves_a_minute_and_no_longer() {
        let t0 = Instant::now();
        let value = fold_flags(None, vec![]);
        store_status(t0, value.clone());

        // Inside the window: the cached value, no recomputation.
        assert_eq!(
            cached_status(t0 + Duration::from_secs(30)).as_ref(),
            Some(&value)
        );
        // At and past the boundary: expired.
        assert!(cached_status(t0 + Duration::from_millis(60_000)).is_none());
        assert!(cached_status(t0 + Duration::from_secs(61)).is_none());

        // The run's drop: a fresh store survives 30s, then invalidate kills
        // even a just-written entry.
        store_status(t0, value.clone());
        invalidate_upgrade_status();
        assert!(cached_status(t0).is_none());

        // Leave the cache empty for whichever test runs next — the cache is
        // process-global and tests run in one process.
        invalidate_upgrade_status();
    }
}
