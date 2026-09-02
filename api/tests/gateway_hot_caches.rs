// Live-DB proof of the hot-path caches (cargo test -- --ignored). Each cache
// buys the gateway its zero-checkout warm turn, and each one has a law the
// pure tests can't pin because the law is about the DATABASE round trip:
// served windows are short, and every in-process writer drops them. A
// regression here is a stale key served for 60s or a revoked identity lasting
// 15s longer than the reset promises — wrong, but silent everywhere except
// here. House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test --test gateway_hot_caches -- --ignored
//
// (TALARIA_SECRET_KEY comes along with the source: the key-cache test seals
// through the same box production resolves through.)

use serde_json::json;
use sqlx::PgPool;
use talaria_api::auth::{authenticate_key, reset_identity_cache, sha256_hex};
use talaria_api::config::Config;
use talaria_api::db::pool as app_pool;
use talaria_api::gateway::provider::{invalidate_endpoint_key, resolve_endpoint_key};
use talaria_api::gateway::registry::{
    add_endpoint_models, create_endpoint, delete_endpoint, invalidate_endpoints_cache,
    list_endpoints,
};
use talaria_api::inbox_focus::conversation::{
    acquire_inbox_focus_lock, get_inbox_conversation, record_inbox_snooze,
};
use talaria_api::inbox_focus::update_focus_state;
use talaria_api::secretbox::SecretBox;
use talaria_api::session::SessionUser;
use talaria_api::state::AppState;

async fn pg() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

/// An AppState on the live database, configured exactly the way the live
/// tests before this one configure one — the secretbox test needs the real
/// root so the box it loads can open what it sealed.
async fn app_state() -> AppState {
    let cfg = Config::from_parts(
        std::env::var("DATABASE_URL").unwrap_or_default(),
        "redis://127.0.0.1:1".into(),
        std::env::var("TALARIA_SECRET_KEY").unwrap_or_default(),
        std::env::var("TALARIA_SECRET_KEY_FILE").unwrap_or_default(),
        String::new(),
        String::new(),
    )
    .expect("test config assembles");
    AppState::new(app_pool(&cfg), std::sync::Arc::new(cfg))
}

/// A user row for the FKs, swept by its distinctive sub. CASCADE covers every
/// child these tests write.
async fn fabricate_user(pg: &PgPool, tag: &str) -> String {
    let sub = format!("hot-caches:{tag}:{}", uuid::Uuid::new_v4());
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
        .bind(format!("hot-caches:{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
}

// ── The endpoints cache ──────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_endpoints_cache_serves_then_drops_on_write() {
    let pg = pg().await;
    let name = format!("hotcache-ep-{}", uuid::Uuid::new_v4().simple());
    // Start cold so the test sees only its own window.
    invalidate_endpoints_cache();

    let id = create_endpoint(
        &pg,
        &SecretBox::default(), // api_key: None — nothing is sealed through it
        &name,
        "openrouter",
        None,
        "cloud",
        None,
        None,
        &["hotcache-model".to_string()],
        &json!({}),
    )
    .await
    .unwrap();
    let served = list_endpoints(&pg).await.unwrap();
    assert!(
        served
            .iter()
            .any(|e| e.id == id && e.models.iter().any(|m| m == "hotcache-model")),
        "a fresh create is visible to the very next completion"
    );

    // add_endpoint_models is a writer: the serve window must drop, not ride
    // out its TTL. (Both lists below happen well inside 15s — without the
    // invalidation hook the second would serve the stale row.)
    add_endpoint_models(&pg, &name, &["hotcache-model-2".to_string()])
        .await
        .unwrap();
    let served = list_endpoints(&pg).await.unwrap();
    assert!(
        served
            .iter()
            .any(|e| e.id == id && e.models.iter().any(|m| m == "hotcache-model-2")),
        "a model added between completions is served to the next one"
    );

    let (deleted, _) = delete_endpoint(&pg, &id).await.unwrap();
    assert!(deleted, "no enabled agent targets a test endpoint");
    let served = list_endpoints(&pg).await.unwrap();
    assert!(
        !served.iter().any(|e| e.id == id),
        "a deleted endpoint is gone from the next completion's routing"
    );
}

// ── The endpoint key cache ───────────────────────────────────────────────────

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_key_cache_pins_within_its_window_and_drops_on_rotation() {
    let state = app_state().await;
    let name = format!("hotcache-key-{}", uuid::Uuid::new_v4().simple());
    // key_env_allowed's shape: *_API_KEY, uppercase only — a uuid's hex is
    // lowercase, so the name upcases it.
    let env_name = format!(
        "TALARIA_HOTCACHE_{}_API_KEY",
        uuid::Uuid::new_v4().simple().to_string().to_uppercase()
    );
    let id = create_endpoint(
        &state.pg,
        &SecretBox::default(),
        &name,
        "openrouter",
        None,
        "cloud",
        Some(&env_name),
        None,
        &[],
        &json!({}),
    )
    .await
    .unwrap();
    let eps = list_endpoints(&state.pg).await.unwrap();
    let ep = eps.iter().find(|e| e.id == id).expect("created").clone();

    // SAFETY: the var name is unique to this test run; no other thread reads it.
    unsafe { std::env::set_var(&env_name, "v1") };
    assert_eq!(
        resolve_endpoint_key(&state, &ep).await.as_deref(),
        Some("v1"),
        "the env fallback resolves and caches"
    );
    // SAFETY: as above.
    unsafe { std::env::set_var(&env_name, "v2") };
    assert_eq!(
        resolve_endpoint_key(&state, &ep).await.as_deref(),
        Some("v1"),
        "within the window the cached key is the answer, even though env moved"
    );
    invalidate_endpoint_key(Some(&id));
    assert_eq!(
        resolve_endpoint_key(&state, &ep).await.as_deref(),
        Some("v2"),
        "the admin key write's invalidation makes the rotation immediate"
    );

    // The sealed column is the source of truth; the cache must serve what it
    // decrypts, not what env says.
    let sb = state.secretbox().await.expect(
        "secretbox loads — TALARIA_SECRET_KEY must match the dev database's (source ui/.env)",
    );
    let cipher = sb.seal("sealed-v1").expect("seal");
    sqlx::query("update llm_endpoints set api_key_cipher = $2 where id::text = $1")
        .bind(&id)
        .bind(&cipher)
        .execute(&state.pg)
        .await
        .unwrap();
    invalidate_endpoint_key(Some(&id));
    assert_eq!(
        resolve_endpoint_key(&state, &ep).await.as_deref(),
        Some("sealed-v1"),
        "a sealed key beats env once the window drops"
    );

    delete_endpoint(&state.pg, &id).await.unwrap();
    invalidate_endpoint_key(Some(&id));
    // SAFETY: as above.
    unsafe { std::env::remove_var(&env_name) };
}

// ── The identity cache ───────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_identity_cache_pins_revocation_until_reset() {
    let pg = pg().await;
    let user = fabricate_user(&pg, "identity").await;
    let secret = format!("tlk_hotcache_{}", uuid::Uuid::new_v4().simple());
    let hash = sha256_hex(&secret);
    sqlx::query(
        "insert into llm_api_keys (user_id, name, key_hash, prefix) \
         values ($1::uuid, 'hotcache', $2, 'tlk_hotc')",
    )
    .bind(&user)
    .bind(&hash)
    .execute(&pg)
    .await
    .unwrap();
    reset_identity_cache();
    assert!(
        authenticate_key(&pg, &secret).await.unwrap().is_some(),
        "a live key authenticates"
    );

    sqlx::query("update llm_api_keys set revoked_at = now() where key_hash = $1")
        .bind(&hash)
        .execute(&pg)
        .await
        .unwrap();
    assert!(
        authenticate_key(&pg, &secret).await.unwrap().is_some(),
        "the documented tradeoff: within the 15s window a revoke rides the TTL"
    );
    reset_identity_cache();
    assert!(
        authenticate_key(&pg, &secret).await.unwrap().is_none(),
        "the in-process reset (revoke_key's hook) is immediate"
    );

    // The negative window: an unknown key stops costing a checkout, and a key
    // minted inside that window stays unknown until the window or the map
    // drops it — both sides of the same law.
    let late = format!("tlk_hotcache_{}", uuid::Uuid::new_v4().simple());
    let late_hash = sha256_hex(&late);
    assert!(authenticate_key(&pg, &late).await.unwrap().is_none());
    sqlx::query(
        "insert into llm_api_keys (user_id, name, key_hash, prefix) \
         values ($1::uuid, 'hotcache-late', $2, 'tlk_hotc')",
    )
    .bind(&user)
    .bind(&late_hash)
    .execute(&pg)
    .await
    .unwrap();
    assert!(
        authenticate_key(&pg, &late).await.unwrap().is_none(),
        "the negative window pins too"
    );
    reset_identity_cache();
    assert!(
        authenticate_key(&pg, &late).await.unwrap().is_some(),
        "and resets with the rest of the map"
    );

    sweep_user_rows(&pg, "identity").await;
    reset_identity_cache();
}

// ── The Inbox lock scoping ───────────────────────────────────────────────────

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_inbox_reads_and_snoozes_under_a_held_turn_lock() {
    let state = app_state().await;
    let user_id = fabricate_user(&state.pg, "inbox").await;
    let user = SessionUser {
        id: user_id.clone(),
        sub: format!("hot-caches:inbox:{user_id}"),
        email: None,
        name: None,
        picture: None,
        role: "member".into(),
        provider: "password".into(),
    };

    // A focus item the snooze can name: a blocked task on the user's board
    // (task_items reads through board_members visibility).
    let board: String = sqlx::query_scalar(
        "insert into boards (name, owner_id) values ('hotcache', $1::uuid) returning id::text",
    )
    .bind(&user_id)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'owner')",
    )
    .bind(&board)
    .bind(&user_id)
    .execute(&state.pg)
    .await
    .unwrap();
    let task: String = sqlx::query_scalar(
        "insert into tasks (board_id, title, status) values ($1::uuid, 'hotcache task', 'blocked') returning id::text",
    )
    .bind(&board)
    .fetch_one(&state.pg)
    .await
    .unwrap();

    // The live turn: the per-user lock, held for the whole (simulated) turn.
    let guard = acquire_inbox_focus_lock(&user.id).expect("lock is free at test start");

    // A streaming assistant row — the mid-turn state the panel polls for.
    let conv: String = sqlx::query_scalar(
        "insert into conversations (user_id, agent_model, kind) values ($1::uuid, 'assistant', 'inbox') returning id::text",
    )
    .bind(&user_id)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into messages (conversation_id, seq, role, content, status) \
         values ($1::uuid, 1, 'assistant', 'hotcache…', 'streaming')",
    )
    .bind(&conv)
    .execute(&state.pg)
    .await
    .unwrap();

    // The read path serves mid-turn: no lock consulted, working flag up.
    let page = get_inbox_conversation(&state, &user, None, None)
        .await
        .expect("the timeline read never consults the turn lock");
    assert_eq!(page.conversation_id, Some(conv));
    assert!(page.working, "a streaming row renders the in-flight reply");

    // The state write path lands under the same held lock.
    let until = "2030-01-01T00:00:00Z";
    assert!(
        update_focus_state(&state.pg, &user, "task", &task, Some(Some(until)), false)
            .await
            .expect("the snooze write never consults the turn lock"),
        "the focus item exists, so the state change lands"
    );
    let entry = record_inbox_snooze(&state, &user, "task", &task, until)
        .await
        .expect("the snooze decision row never consults the turn lock");
    assert!(
        entry.is_some(),
        "the timeline entry comes back for the card"
    );

    drop(guard);
    assert!(
        acquire_inbox_focus_lock(&user.id).is_some(),
        "the lock itself still works — release on drop, one turn at a time"
    );

    sweep_user_rows(&state.pg, "inbox").await;
}
