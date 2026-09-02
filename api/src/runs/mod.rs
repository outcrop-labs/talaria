// The runs engine. Dependency-ordered: the lease primitive everything stands
// on, the `runs` store (every write a CAS), the definitions/state machine, the
// driver (enqueue/drive/cancel), the awaiting decision path, and the reclaim
// sweep the scheduler drives.

pub mod decide;
pub mod define;
pub mod defs;
pub mod lease;
pub mod reclaim;
pub mod run;
pub mod store;

use std::sync::Arc;

use crate::approvals::{ApprovalDeps, announce_approval, audience_for, run_decision_approval};
use crate::notify::{NotifyDeps, mark_brief_stale};
use crate::realtime::{self, RealtimeDeps};
use crate::runs::decide::{
    AnnounceFn, ApprovalForFn, AudienceForFn, DecideDeps, MarkBriefStaleFn, PauseDeps, pause_fn,
};
use crate::runs::define::{RunRow, run_definition};
use crate::runs::run::{DefinitionForFn, NowFn, PublishFn, RedisRunLease, RunDeps};
use crate::runs::store::PgRunStore;

/// The real edges two assemblies share: everything `RunDeps` and `DecideDeps`
/// each need a copy of is built HERE and exactly once. The pause a driver
/// parks a question through and the decision that answers it must resolve the
/// same audience and announce through the same key — two separately-built
/// copies of those closures would still work today and drift silently the day
/// one of them grows a condition, which is the failure this shape exists to
/// make impossible.
struct RealEdges {
    store: Arc<PgRunStore>,
    publish: PublishFn,
    definition_for: DefinitionForFn,
    now: NowFn,
    /// Who may be told about a parked question — the one resolver, wrapped to
    /// the edge shape the pause wants. Its errors already resolve to nobody
    /// inside `audience_for`; this wrapper only moves the pool into the future.
    audience_for: AudienceForFn,
    /// The announcer `pause` rides after the park: approve through the same
    /// `announce_approval` the raiser calls, so a question is announced exactly
    /// once from whichever path reached it first. It answers 0 on every
    /// internal failure and leaves the key unmarked for the sweep.
    announce: AnnounceFn,
}

fn real_edges(pg: sqlx::PgPool, rt: RealtimeDeps) -> RealEdges {
    let now: NowFn = Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    });
    let store = Arc::new(PgRunStore::new(pg.clone()));
    let publish: PublishFn = realtime::run_publish(rt.clone());
    let definition_for: DefinitionForFn = Arc::new(run_definition);

    let audience_pg = pg.clone();
    let audience_for: AudienceForFn = Arc::new(move |authority| {
        let pg = audience_pg.clone();
        let authority = authority.clone();
        Box::pin(async move { audience_for(&pg, &authority).await })
    });

    let announce_deps = ApprovalDeps::new(pg, rt, definition_for.clone(), now.clone());
    let announce: AnnounceFn = Arc::new(move |key| {
        let deps = announce_deps.clone();
        let key = key.to_string();
        Box::pin(async move { announce_approval(&deps, &key).await })
    });

    RealEdges {
        store,
        publish,
        definition_for,
        now,
        audience_for,
        announce,
    }
}

/// The full real assembly: the store, the lease, the realtime publish, the
/// registry lookup, and the REAL pause, wired through decide's `pause_fn` to
/// the approvals plane's one resolver and one announcer.
///
/// It is what the scheduler's `run-reclaim` job drives reclaimed runs through,
/// and what work_dispatch's `dispatch_deps` returns. A driven run that parks
/// a question actually parks it.
pub fn real_run_deps(
    pg: sqlx::PgPool,
    redis: redis::aio::ConnectionManager,
    rt: RealtimeDeps,
) -> RunDeps {
    let RealEdges {
        store,
        publish,
        definition_for,
        now,
        audience_for,
        announce,
    } = real_edges(pg, rt);
    RunDeps {
        pause: pause_fn(PauseDeps {
            store: store.clone(),
            publish: publish.clone(),
            definition_for: definition_for.clone(),
            audience_for,
            announce,
        }),
        store,
        lease: Arc::new(RedisRunLease::new(redis)),
        publish,
        definition_for,
        now,
        new_id: Arc::new(|| uuid::Uuid::new_v4().to_string()),
    }
}

/// The answer half of the same assembly: what `decide` runs with when a
/// person resolves a parked question through a route. Its `run` is the full
/// RunDeps (a decided run with `start` may resume driving), and its
/// audience/announce edges are the same closures the pause parks through —
/// built once, shared, so the question and its answer cannot disagree about
/// who was involved.
///
/// THE ROW→APPROVAL TRANSLATION rides along as `approval_for`: a decided run
/// may owe its askers a resolution ping, and the shape is the approvals
/// plane's, not a route's.
pub fn real_decide_deps(
    pg: sqlx::PgPool,
    redis: redis::aio::ConnectionManager,
    rt: RealtimeDeps,
) -> DecideDeps {
    let RealEdges {
        store,
        publish,
        definition_for,
        now,
        audience_for,
        announce,
    } = real_edges(pg.clone(), rt.clone());
    let run = RunDeps {
        pause: pause_fn(PauseDeps {
            store: store.clone(),
            publish: publish.clone(),
            definition_for: definition_for.clone(),
            audience_for: audience_for.clone(),
            announce: announce.clone(),
        }),
        store,
        lease: Arc::new(RedisRunLease::new(redis)),
        publish,
        definition_for: definition_for.clone(),
        now,
        new_id: Arc::new(|| uuid::Uuid::new_v4().to_string()),
    };

    let approval_definition = definition_for.clone();
    let approval_for: ApprovalForFn =
        Arc::new(move |run: &RunRow| run_decision_approval(run, &approval_definition));

    // A decision is not undone because a brief refused to be marked: the
    // answer is written, the run may already be moving, and the brief sweeps
    // again on its own throttle. Logged, never propagated.
    let brief_deps = NotifyDeps { pg, realtime: rt };
    let mark_brief_stale: MarkBriefStaleFn = Arc::new(move |user_ids| {
        let deps = brief_deps.clone();
        let user_ids = user_ids.clone();
        Box::pin(async move {
            if let Err(e) = mark_brief_stale(&deps, &user_ids).await {
                tracing::error!(
                    "runs: could not mark briefs stale for {} deciders: {e}",
                    user_ids.len()
                );
            }
        })
    });

    DecideDeps {
        run,
        approval_for,
        audience_for,
        announce,
        mark_brief_stale,
    }
}
