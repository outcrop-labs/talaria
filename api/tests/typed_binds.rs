// Live-DB proof of the typed-bind fixes (cargo test -- --ignored). The port's
// crash class: sqlx declares each bind's wire type from the Rust value
// (String → TEXT), so a text bind COMPARED against a non-text column dies at
// prepare with `operator does not exist` — and every carrier here either
// 500'd or swallowed the error and read nothing. Each test executes the real
// statement against the real schema, because that is the only layer the bug
// lived on. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test --test typed_binds -- --ignored
//
// The sweep test also needs the dev retrieval containers (talaria-qdrant-dev,
// talaria-embeddings-dev) up — it proves the window queries READ, which the
// health gate fronts.

use serde_json::Value;
use sqlx::postgres::PgPool;
use talaria_api::agent_auth::epoch_ms_to_iso;
use talaria_api::daily_brief::load_recent_row;
use talaria_api::retrieval::backfill::{rag_health, sweep_new_activity};
use talaria_api::retrieval::embed::real_deps as real_embed_deps;
use talaria_api::retrieval::qdrant::real_deps as real_qdrant_deps;
use talaria_api::retrieval::sources::unindex_activity;
use talaria_api::runs::defs::reindex::artifact_links_for;

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// A user row for the FKs, swept by its distinctive sub. CASCADE covers every
/// child these tests write.
async fn fabricate_user(pg: &PgPool, tag: &str) -> String {
    let sub = format!("typed-binds:{tag}:{}", uuid::Uuid::new_v4());
    sqlx::query("insert into users (sub) values ($1)")
        .bind(&sub)
        .execute(pg)
        .await
        .unwrap();
    let id: String = sqlx::query_scalar("select id::text from users where sub = $1")
        .bind(&sub)
        .fetch_one(pg)
        .await
        .unwrap();
    id
}

async fn sweep_user_rows(pg: &PgPool, tag: &str) {
    sqlx::query("delete from users where sub like $1")
        .bind(format!("typed-binds:{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
}

/// The "yesterday's brief is still the current one" read. Pre-fix it 500'd on
/// every hit: the ISO cutoff bound as TEXT against `created_at > $2` with no
/// cast, a prepare-time operator error. The cast rides the bind now; the test
/// proves both that the statement parses AND that the 48h window still
/// compares as time, not as text.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_brief_recency_read_finds_a_fresh_brief_and_expires_a_stale_one() {
    let pg = pool().await;
    let user_id = fabricate_user(&pg, "brief").await;

    sqlx::query(
        "insert into daily_briefs (user_id, brief_date, last_seq) \
         values ($1::uuid, current_date, 1)",
    )
    .bind(&user_id)
    .execute(&pg)
    .await
    .unwrap();

    // Today's date from the same source the row used — no timezone arithmetic
    // in the test to get wrong.
    let today: String = sqlx::query_scalar("select to_char(current_date, 'YYYY-MM-DD')")
        .fetch_one(&pg)
        .await
        .unwrap();

    // An hour old: inside the window.
    sqlx::query(
        "update daily_briefs set created_at = now() - interval '1 hour' where user_id = $1::uuid",
    )
    .bind(&user_id)
    .execute(&pg)
    .await
    .unwrap();
    let fresh = load_recent_row(&pg, &user_id, now_ms()).await.unwrap();
    let row = fresh.expect("a one-hour-old brief with entries is the current one");
    assert_eq!(row.user_id, user_id);
    assert_eq!(row.brief_date, today);
    assert_eq!(row.last_seq, 1);

    // 49 hours old: outside the window. The Some case above proves the
    // statement parses; this one proves the cast didn't lobotomize the
    // comparison into always-true — the window still excludes old rows.
    sqlx::query(
        "update daily_briefs set created_at = now() - interval '49 hours' where user_id = $1::uuid",
    )
    .bind(&user_id)
    .execute(&pg)
    .await
    .unwrap();
    let stale = load_recent_row(&pg, &user_id, now_ms()).await.unwrap();
    assert!(
        stale.is_none(),
        "a 49-hour-old brief is nobody's current one"
    );

    sweep_user_rows(&pg, "brief").await;
}

/// The reindex page's link read. Pre-fix `artifact_id = any($1)` bound the
/// page's text ids as TEXT[] against a uuid column — prepare error, page
/// dead whenever an artifact existed. The cast rides the bind now.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_reindex_page_reads_link_rows_for_its_artifacts() {
    let pg = pool().await;
    let artifact_id = uuid::Uuid::new_v4().to_string();
    sqlx::query("insert into artifacts (id, kind, title, body) values ($1::uuid, 'doc', 'link probe', 'body text')")
        .bind(&artifact_id)
        .execute(&pg)
        .await
        .unwrap();
    for target_type in ["plan", "research"] {
        sqlx::query(
            "insert into artifact_links (artifact_id, target_type, target_id) \
             values ($1::uuid, $2, $3)",
        )
        .bind(&artifact_id)
        .bind(target_type)
        .bind(uuid::Uuid::new_v4().to_string())
        .execute(&pg)
        .await
        .unwrap();
    }
    // A link the page does not want — the type filter must drop it, not read it.
    sqlx::query(
        "insert into artifact_links (artifact_id, target_type, target_id) \
         values ($1::uuid, 'task', $2)",
    )
    .bind(&artifact_id)
    .bind(uuid::Uuid::new_v4().to_string())
    .execute(&pg)
    .await
    .unwrap();

    let links = artifact_links_for(&pg, std::slice::from_ref(&artifact_id))
        .await
        .unwrap();
    let mut types: Vec<&str> = links.iter().map(|(_, t, _)| t.as_str()).collect();
    types.sort();
    assert_eq!(
        types,
        vec!["plan", "research"],
        "both wanted links, no others"
    );
    assert!(links.iter().all(|(a, _, _)| *a == artifact_id));

    // The empty page — the first page of an empty install — must be a clean
    // no-row read, not an `any('{}')` error.
    assert!(artifact_links_for(&pg, &[]).await.unwrap().is_empty());

    sqlx::query("delete from artifacts where id = $1::uuid")
        .bind(&artifact_id)
        .execute(&pg)
        .await
        .unwrap();
}

/// The 15-minute RAG sweep. Pre-fix all four window queries died at prepare
/// (`updated_at > $1`, TEXT vs timestamptz) and `.unwrap_or_default()` ate
/// the error — the sweep "ran", indexed nothing, and still advanced its
/// watermark, silently burning every window since the cutover. The casts ride
/// the binds now; the test proves the sweep READS its window and bumps the
/// mark, against the real retrieval services.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_rag_sweep_reads_its_window_and_advances_its_watermark() {
    let pg = pool().await;
    let qd = real_qdrant_deps();
    let ed = real_embed_deps();
    let health = rag_health(&qd, &ed).await;
    assert!(
        health.qdrant && health.embeddings,
        "start the dev retrieval containers (talaria-qdrant-dev, \
         talaria-embeddings-dev) to run this test — saw {health:?}"
    );

    let user_id = fabricate_user(&pg, "sweep").await;
    let board_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "insert into boards (id, name, owner_id) values ($1::uuid, 'sweep probe board', $2::uuid)",
    )
    .bind(&board_id)
    .bind(&user_id)
    .execute(&pg)
    .await
    .unwrap();
    let task_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "insert into tasks (id, board_id, title, description) \
         values ($1::uuid, $2::uuid, $3, 'sweep probe description')",
    )
    .bind(&task_id)
    .bind(&board_id)
    .bind(format!("sweep probe {task_id}"))
    .execute(&pg)
    .await
    .unwrap();

    // The watermark key is backfill.rs's SWEEP_KEY. Backdated one minute so
    // the window holds the probe task and (barring a busy dev box) nothing
    // else; the sweep's own closing write leaves the mark at "now", which is
    // exactly where the production sweep leaves it every fifteen minutes —
    // content hashes make any overlap free, so the mark is left there.
    let mark = epoch_ms_to_iso(now_ms() - 60_000);
    talaria_api::gateway::settings::set_setting(
        &pg,
        "rag_sweep_watermark",
        &Value::String(mark.clone()),
    )
    .await
    .unwrap();

    let indexed = sweep_new_activity(&pg, &qd, &ed).await;
    assert!(
        indexed >= 1,
        "the sweep must read the task its window holds — read nothing instead"
    );

    // The mark must have MOVED — pre-fix it advanced over windows that were
    // never read; the point of the fix is that it now advances over windows
    // that WERE. The exact stamp is the sweep's start-of-run clock, so all
    // the test can pin is that it changed.
    let after = talaria_api::gateway::settings::get_setting(
        &pg,
        "rag_sweep_watermark",
        Value::String(String::new()),
    )
    .await;
    assert_ne!(after.as_str().unwrap_or(""), mark);

    // The probe leaves no trace: point out of the brain, rows out of the DB.
    let _ = unindex_activity(&pg, &qd, &ed, "ticket", &task_id).await;
    sweep_user_rows(&pg, "sweep").await;
}

/// The board status patch — the "edit where agents start" write. Pre-fix every
/// field edit on a column 500'd: the row id is resolved by key and read back
/// `id::text`, then the dynamic `update board_statuses set … where id = $N`
/// bound that string as TEXT against the uuid column — prepare-time operator
/// error, on every patch, first edit to the last. The cast rides the final bind
/// now; the test drives the real `update_status` (materialize, then the dynamic
/// statement) and proves the row actually moved.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_status_patch_lands_through_the_dynamic_update() {
    let pg = pool().await;
    let user_id = fabricate_user(&pg, "status-patch").await;
    let board_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "insert into boards (id, name, owner_id) values ($1::uuid, 'patch probe board', $2::uuid)",
    )
    .bind(&board_id)
    .bind(&user_id)
    .execute(&pg)
    .await
    .unwrap();

    let deps = talaria_api::tasks::TaskDeps::from_route(pg.clone(), None);
    async fn read_back(pg: &sqlx::PgPool, board_id: &str, key: &str) -> (String, bool) {
        sqlx::query_as(
            "select label, agent_start from board_statuses \
             where board_id = $1::uuid and key = $2",
        )
        .bind(board_id)
        .bind(key)
        .fetch_one(pg)
        .await
        .unwrap()
    }

    // First touch materializes the defaults; this patch then runs the dynamic
    // UPDATE the same request would.
    talaria_api::statuses::update_status(
        &deps,
        &board_id,
        "in_progress",
        &talaria_api::statuses::StatusPatch {
            label: Some("Underway".into()),
            color: None,
            category: None,
            agent_start: Some(true),
            position: None,
        },
        "typed-binds",
    )
    .await
    .unwrap()
    .expect("the patch is a legal write on an active column");
    let (label, agent_start) = read_back(&pg, &board_id, "in_progress").await;
    assert_eq!(label, "Underway");
    assert!(agent_start, "agentStart=true must land, not just parse");

    // A SECOND patch — different fields, same statement shape — so the test
    // cannot pass on materialize's INSERT alone.
    talaria_api::statuses::update_status(
        &deps,
        &board_id,
        "in_progress",
        &talaria_api::statuses::StatusPatch {
            label: None,
            color: Some("teal".into()),
            category: None,
            agent_start: Some(false),
            position: None,
        },
        "typed-binds",
    )
    .await
    .unwrap()
    .expect("the second patch lands too");
    let (label, agent_start) = read_back(&pg, &board_id, "in_progress").await;
    assert_eq!(label, "Underway", "the second patch left the label alone");
    assert!(
        !agent_start,
        "agentStart=false must land through the same path"
    );

    sweep_user_rows(&pg, "status-patch").await;
}
