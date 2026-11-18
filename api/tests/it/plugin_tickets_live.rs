// plugin_tickets_live (see main.rs): a plugin's own tickets still start their
// maintainer. Assigning a ticket no longer moves it into the agent column, so
// a caller whose ticket IS a person's ask for an agent's work names that
// column itself (talaria_tasks::pickup_column). A plugin change request — a
// person asking the maintainer from Manage → Apps — is born in the board's
// pickup column and dispatches the maintainer, exactly as before; a ticket
// created the same way with no column chosen lands in intake and starts
// nothing.
//
// House rule: #[ignore]d, never CI's default job.
//
//   DATABASE_URL=postgres://… REDIS_URL=redis://… TALARIA_SECRET_KEY=… \
//     cargo test -p talaria-api --test it plugin_tickets_live:: -- --ignored

use sqlx::PgPool;

use crate::support::live_app_state;

const PREFIX: &str = "plugin-tickets-live";

async fn sessions(pg: &PgPool, id: &str) -> i64 {
    sqlx::query_scalar(
        "select count(*)::bigint from runs where kind = 'work-session' and subject_id = $1",
    )
    .bind(id)
    .fetch_one(pg)
    .await
    .unwrap()
}

/// The count once the create's detached push has landed (polled, 15 s cap).
async fn sessions_reach(pg: &PgPool, id: &str, want: i64) -> i64 {
    for _ in 0..150 {
        if sessions(pg, id).await >= want {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    sessions(pg, id).await
}

#[tokio::test]
#[ignore]
async fn a_change_request_is_born_in_the_pickup_column_and_starts_its_maintainer() {
    crate::support::wire_boot_seams();
    let state = live_app_state().await;
    let pg = state.pg.clone();
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let email = format!("{PREFIX}-{tag}@plugin-test.invalid");
    let (user,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $1, 'Plugin Owner', 'member') returning id::text",
    )
    .bind(&email)
    .fetch_one(&pg)
    .await
    .unwrap();
    let (board,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id) values ('Plugin board', $1::uuid) returning id::text",
    )
    .bind(&user)
    .fetch_one(&pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'owner')",
    )
    .bind(&board)
    .bind(&user)
    .execute(&pg)
    .await
    .unwrap();
    let model = format!("{PREFIX}-{tag}");
    let (agent_id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name) \
         values ($1, 'personal', $1, 'Plugin Maintainer') returning id::text",
    )
    .bind(&model)
    .fetch_one(&pg)
    .await
    .unwrap();
    sqlx::query("insert into board_agents (board_id, agent_model) values ($1::uuid, $2)")
        .bind(&board)
        .bind(&model)
        .execute(&pg)
        .await
        .unwrap();

    // The real push side: the dispatch assembly, and the seam the process
    // wires at boot (talaria-jobs).
    let redis = state
        .redis()
        .await
        .expect("redis connects (source ui/.env)");
    let rt = talaria_realtime::RealtimeDeps::publish_only(Some(redis.clone()));
    let run = talaria_work_dispatch::dispatch_deps(pg.clone(), redis, rt);
    let _ = talaria_tasks_types::MAYBE_DISPATCH_TICKET.set(std::sync::Arc::new(
        |pg, dispatch, ticket, only_agents| {
            Box::pin(async move {
                let Some(deps) = dispatch else { return };
                talaria_work_dispatch::maybe_dispatch_ticket(
                    &pg,
                    &deps,
                    &ticket,
                    only_agents.as_deref(),
                )
                .await;
            })
        },
    ));
    let mut deps = talaria_tasks::TaskDeps::from_route(pg.clone(), None);
    deps.dispatch = Some(run);

    let plugin = talaria_plugins::Plugin {
        app_slug: format!("{PREFIX}-{tag}"),
        git_host_id: None,
        repo: "acme/notes".into(),
        url: "https://example.invalid/acme/notes".into(),
        host: "example.invalid".into(),
        branch: None,
        installed_commit: None,
        maintainer_agent_id: Some(agent_id),
        maintainer_model: Some(model.clone()),
        maintainer_name: Some("Plugin Maintainer".into()),
        owner_user_id: Some(user.clone()),
        owner_name: Some("Plugin Owner".into()),
        board_id: Some(board.clone()),
        board_name: Some("Plugin board".into()),
        task_id: None,
        ticket_ref: None,
        update_policy: "manual".into(),
        created_at: String::new(),
    };
    let filed = talaria_plugins::change::request_change(
        &deps,
        &plugin,
        "Add tags to notes",
        &user,
        "Plugin Owner",
    )
    .await
    .unwrap_or_else(|e| panic!("request_change: {e}"));
    let (status,): (String,) = sqlx::query_as("select status from tasks where id = $1::uuid")
        .bind(&filed.task_id)
        .fetch_one(&pg)
        .await
        .unwrap();
    let pickup = talaria_tasks::pickup_column(&pg, &board).await.unwrap();
    assert_eq!(
        pickup.as_deref(),
        Some("assigned"),
        "the default board's pickup column"
    );
    assert_eq!(
        Some(status.as_str()),
        pickup.as_deref(),
        "the change request left the pickup column"
    );
    assert_eq!(
        sessions_reach(&pg, &filed.task_id, 1).await,
        1,
        "the change request did not start its maintainer"
    );

    // The same assignee on an ordinary create, no column chosen: intake, and
    // nothing starts — assignment alone never moves a ticket or starts work.
    let plain = talaria_tasks::create_task(
        &deps,
        &talaria_tasks::NewTask {
            board_id: &board,
            title: "Assigned, not placed",
            description: None,
            priority: None,
            effort: None,
            assignees: std::slice::from_ref(&model),
            due_date: None,
            start_date: None,
            color: None,
            estimated_hours: None,
            parent_id: None,
            tags: &[],
            created_by: &email,
            status: None,
        },
    )
    .await
    .unwrap_or_else(|e| panic!("create: {}", e.message()));
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    assert_eq!(plain.status, "inbox");
    assert_eq!(
        sessions(&pg, &plain.id).await,
        0,
        "an assigned, unplaced ticket started"
    );

    sqlx::query("delete from runs where kind = 'work-session' and input->>'agentModel' = $1")
        .bind(&model)
        .execute(&pg)
        .await
        .unwrap();
    sqlx::query("delete from agent_defs where model = $1")
        .bind(&model)
        .execute(&pg)
        .await
        .unwrap();
    sqlx::query("delete from users where id = $1::uuid")
        .bind(&user)
        .execute(&pg)
        .await
        .unwrap();
}
