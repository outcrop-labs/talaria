// The runs engine — port of ui/src/server/runs/** (batch 4). One module per
// TS file, dependency-ordered: the lease primitive everything stands on, the
// `runs` store (every write a CAS), the definitions/state machine, the driver
// (enqueue/drive/cancel), the awaiting decision path, and the reclaim sweep.
// The registered scheduler that drives reclaim and the run kinds land later in
// the batch; until the handoff slice, nothing here is armed.

pub mod decide;
pub mod define;
pub mod defs;
pub mod lease;
pub mod reclaim;
pub mod run;
pub mod store;

use std::sync::Arc;

use crate::approvals::{ApprovalDeps, announce_approval, audience_for};
use crate::realtime::{self, RealtimeDeps};
use crate::runs::decide::{AnnounceFn, AudienceForFn, PauseDeps, pause_fn};
use crate::runs::define::run_definition;
use crate::runs::run::{DefinitionForFn, NowFn, PublishFn, RedisRunLease, RunDeps};
use crate::runs::store::PgRunStore;

/// The full real assembly: the store, the lease, the realtime publish, the
/// registry lookup, and — the piece the coexistence assembly in work_dispatch
/// stubs out — the REAL pause, wired through decide's `pause_fn` to the
/// approvals plane's one resolver and one announcer.
///
/// This is the handoff assembly the codebase's comments keep promising: it is
/// what the Rust scheduler's `run-reclaim` job drives reclaimed runs through,
/// and what work_dispatch's `coexistence_dispatch_deps` is replaced by when
/// the flip lands (during coexistence nothing in Rust drives, so the stub's
/// loud refusal was the honest edge; from the flip on, a driven run that
/// parks a question must actually park it).
///
/// Nothing here is reached before the flip: the job that consumes it arms
/// with `start_scheduler`, and the flip is the only caller of that.
pub fn real_run_deps(
    pg: sqlx::PgPool,
    redis: redis::aio::ConnectionManager,
    rt: RealtimeDeps,
) -> RunDeps {
    let now: NowFn = Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    });
    let store = Arc::new(PgRunStore::new(pg.clone()));
    let publish: PublishFn = realtime::run_publish(rt.clone());
    let definition_for: DefinitionForFn = Arc::new(run_definition);

    // Who may be told about a parked question — the one resolver, wrapped to
    // the edge shape the pause wants. Its errors already resolve to nobody
    // inside `audience_for`; this wrapper only moves the pool into the future.
    let audience_pg = pg.clone();
    let audience_for: AudienceForFn = Arc::new(move |authority| {
        let pg = audience_pg.clone();
        let authority = authority.clone();
        Box::pin(async move { audience_for(&pg, &authority).await })
    });

    // The announcer `pause` rides after the park: approve through the same
    // `announce_approval` the raiser calls, so a question is announced exactly
    // once from whichever path reached it first. It answers 0 on every
    // internal failure and leaves the key unmarked for the sweep — the same
    // contract TS's catch-all wrapper gave.
    let announce_deps = ApprovalDeps::new(pg, rt, definition_for.clone(), now.clone());
    let announce: AnnounceFn = Arc::new(move |key| {
        let deps = announce_deps.clone();
        let key = key.to_string();
        Box::pin(async move { announce_approval(&deps, &key).await })
    });

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
