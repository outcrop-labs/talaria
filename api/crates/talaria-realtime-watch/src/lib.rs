// Real-time fan-out: Redis/SSE engine in talaria-realtime.
// Run ACL wiring stays here (store / channels / conversations / tasks).
pub use talaria_realtime::*;

use futures_util::FutureExt;
use std::sync::Arc;
use talaria_runs_run::{PublishFn as RunPublishFn, RunEvent};
use talaria_runs_store::RunStore;

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
                let store = talaria_runs_store::PgRunStore::new(pg);
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
                talaria_boards::board_role(&pg, &user_id, &board_id)
                    .await
                    .map_err(|e| e.to_string())
            }
            .boxed()
        }),
        channel_role: Arc::new(move |user_id, channel_id| {
            let pg = channel_pg.clone();
            async move {
                talaria_channels::channel_role(&pg, &user_id, &channel_id)
                    .await
                    .map_err(|e| e.to_string())
            }
            .boxed()
        }),
        task_board_id: Arc::new(move |task_id| {
            let pg = task_pg.clone();
            async move {
                // Asked here, directly. This used to indirect through a
                // TASK_BOARD_ID OnceLock that NOTHING EVER SET, so it
                // answered None for every task — and None on this edge means
                // NotAudience, which the routes render as 403. Every run
                // whose subject is a ticket was therefore unwatchable by
                // everyone, permanently and silently: the agent pane, the
                // Turns transcript and the run's own event stream all refuse
                // together, because all three gate on may_watch_run.
                //
                // The indirection bought nothing. It reads as a
                // dependency-cycle break, but this module already calls
                // talaria_boards, talaria_channels, talaria_conversations
                // and talaria_users directly on the edges either side of
                // this one, and the question is one column.
                let row: Option<(String,)> =
                    sqlx::query_as("select board_id::text from tasks where id = $1::uuid")
                        .bind(&task_id)
                        .fetch_optional(&pg)
                        .await
                        .map_err(|e| e.to_string())?;
                Ok(row.map(|(board_id,)| board_id))
            }
            .boxed()
        }),
        conversation_access: Arc::new(move |user_id, conversation_id| {
            let pg = conversation_pg.clone();
            async move {
                talaria_conversations::conversation_accessible(&pg, &user_id, &conversation_id)
                    .await
                    .map_err(|e| e.to_string())
            }
            .boxed()
        }),
        is_admin: Arc::new(move |user_id| {
            let pg = admin_pg.clone();
            async move {
                talaria_users::get_user_role(&pg, &user_id)
                    .await
                    .map(|role| role == "admin")
                    .map_err(|e| e.to_string())
            }
            .boxed()
        }),
    }
}
