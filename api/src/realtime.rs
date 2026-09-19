// Real-time fan-out: Redis/SSE engine in talaria-realtime.
// Run ACL wiring stays here (store / channels / conversations / tasks).
pub use talaria_realtime::*;

use crate::runs::run::{PublishFn as RunPublishFn, RunEvent};
use crate::runs::store::RunStore;
use futures_util::FutureExt;
use std::sync::Arc;

/// The real `RunDeps.publish` assembly the driver's deps point at.
pub fn run_publish(deps: RealtimeDeps) -> RunPublishFn {
    Arc::new(move |event: RunEvent, owner_user_id: Option<&str>| {
        publish_run(
            &deps,
            &event.run_id,
            &RunWireEvent {
                kind_tag: "run",
                run_id: event.run_id.clone(),
                kind: event.kind.clone(),
                state: event.state,
                phase: event.phase.clone(),
                error: event.error.clone(),
            },
        );
        if let Some(owner) = owner_user_id.filter(|o| !o.is_empty()) {
            publish_user(
                &deps,
                owner,
                &UserEvent::Run {
                    run_id: event.run_id.clone(),
                    state: event.state,
                },
            );
        }
    })
}

/// The real watch edges over the pool. Each returns the sqlx error text in
/// the `Err` string — the route logs it and answers 500.
pub fn real_watch_deps(pg: sqlx::PgPool) -> RunWatchDeps {
    // One clone per edge: every closure owns its pool handle outright.
    let get_run_pg = pg.clone();
    let board_pg = pg.clone();
    let channel_pg = pg.clone();
    let task_pg = pg.clone();
    let conversation_pg = pg.clone();
    let admin_pg = pg;
    RunWatchDeps {
        get_run: Arc::new(move |id| {
            let pg = get_run_pg.clone();
            async move {
                let store = crate::runs::store::PgRunStore::new(pg);
                let row = store
                    .get(&id)
                    .await
                    .map_err(|e| e.to_string())?
                    .map(|r| RunWatchRow {
                        owner_user_id: r.owner_user_id,
                        subject_type: r.subject_type,
                        subject_id: r.subject_id,
                    });
                Ok(row)
            }
            .boxed()
        }),
        board_role: Arc::new(move |user_id, board_id| {
            let pg = board_pg.clone();
            async move {
                crate::boards::board_role(&pg, &user_id, &board_id)
                    .await
                    .map_err(|e| e.to_string())
            }
            .boxed()
        }),
        channel_role: Arc::new(move |user_id, channel_id| {
            let pg = channel_pg.clone();
            async move {
                crate::channels::channel_role(&pg, &user_id, &channel_id)
                    .await
                    .map_err(|e| e.to_string())
            }
            .boxed()
        }),
        task_board_id: Arc::new(move |task_id| {
            let pg = task_pg.clone();
            async move {
                crate::tasks::task_board_id(&pg, &task_id)
                    .await
                    .map_err(|e| e.to_string())
            }
            .boxed()
        }),
        conversation_access: Arc::new(move |user_id, conversation_id| {
            let pg = conversation_pg.clone();
            async move {
                crate::conversations::conversation_accessible(&pg, &user_id, &conversation_id)
                    .await
                    .map_err(|e| e.to_string())
            }
            .boxed()
        }),
        is_admin: Arc::new(move |user_id| {
            let pg = admin_pg.clone();
            async move {
                crate::users::get_user_role(&pg, &user_id)
                    .await
                    .map(|role| role == "admin")
                    .map_err(|e| e.to_string())
            }
            .boxed()
        }),
    }
}
