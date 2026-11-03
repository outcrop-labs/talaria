// Live-DB proof of the Hermes stable channel's database half
// (cargo test -- --ignored): the per-digest records (`hermes_images`), the
// per-agent assignments (`hermes_agent_images`) and the in-use probe the
// rollout passes busy agents over with. The planning itself is a pure
// function with unit tests (talaria-hermes-images `plan_from`); what only
// Postgres can vouch for is the SQL — the casts, the upserts' answers, the
// cascade, and that each busy signal is read from the column it names.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-api --test it hermes_channel_live:: -- --ignored
//
// It never touches the channel's own settings row (`hermes_channel`), so it
// is safe beside a running stack.

use crate::support::pg;
use sqlx::postgres::PgPool;
use talaria_hermes_channel::busy::busy;
use talaria_hermes_images as images;

const PREFIX: &str = "hermes-channel-test";

fn digest(tag: &str, n: u8) -> String {
    // 64 lowercase hex: the tag's bytes, padded — a digest no registry names.
    let mut hex: String = format!("{tag}{n}")
        .bytes()
        .map(|b| format!("{:02x}", b))
        .collect();
    hex.truncate(56);
    format!("sha256:{hex:0<64}")
}

async fn cleanup(pg: &PgPool, tag: &str) {
    for n in 0..4 {
        sqlx::query("delete from hermes_images where digest = $1")
            .bind(digest(tag, n))
            .execute(pg)
            .await
            .unwrap();
    }
    sqlx::query("delete from agent_defs where slug like $1")
        .bind(format!("{PREFIX}-{tag}-%"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from users where sub like $1")
        .bind(format!("{PREFIX}:{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
}

async fn agent(pg: &PgPool, tag: &str, name: &str) -> (String, String) {
    let slug = format!("{PREFIX}-{tag}-{name}");
    let model = format!("{slug}-model");
    let id: String = sqlx::query_scalar(
        "insert into agent_defs (slug, department, model, display_name) \
         values ($1, $1, $2, $1) returning id::text",
    )
    .bind(&slug)
    .bind(&model)
    .fetch_one(pg)
    .await
    .unwrap();
    (id, model)
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_digest_is_seen_once_prepared_once_and_bad_for_good() {
    let pg = pg().await;
    let tag = "rec";
    cleanup(&pg, tag).await;
    let d = digest(tag, 0);
    assert!(images::is_digest(&d), "{d}");

    // First sighting is new; the next is not, and keeps the version a
    // sighting without one does not know.
    assert!(
        images::note_seen(&pg, &d, "nousresearch/hermes-agent", Some("v0.22.6"))
            .await
            .unwrap()
    );
    assert!(
        !images::note_seen(&pg, &d, "nousresearch/hermes-agent", None)
            .await
            .unwrap()
    );
    // An untrusted tag is never kept.
    assert!(
        !images::note_seen(&pg, &d, "nousresearch/hermes-agent", Some("stable"))
            .await
            .unwrap()
    );
    let r = images::record(&pg, &d).await.unwrap().expect("recorded");
    assert_eq!(r.version.as_deref(), Some("v0.22.6"));
    assert_eq!(r.status, images::Status::Candidate);
    assert!(!r.runnable(), "seen is not prepared");

    // Prepared: the image's own config version, its stripped names, pulled
    // here; the image's self-reported version does not replace the tag's.
    let inspect =
        serde_json::json!({ "strippedEnv": ["OPENAI_API_KEY", "not a name"], "s6": true });
    images::set_prepared(&pg, &d, 52, &inspect, true, Some("v9.9.9"))
        .await
        .unwrap();
    let r = images::record(&pg, &d).await.unwrap().unwrap();
    assert_eq!((r.config_version, r.pulled_here), (Some(52), true));
    assert!(r.runnable());
    assert_eq!(r.version.as_deref(), Some("v0.22.6"));
    assert_eq!(r.stripped_env(), vec!["OPENAI_API_KEY"]);
    assert_eq!(
        r.image_ref(),
        format!("nousresearch/hermes-agent:v0.22.6@{d}")
    );

    // A digest with no tag takes the image's own version.
    let untagged = digest(tag, 1);
    images::note_seen(&pg, &untagged, "nousresearch/hermes-agent", None)
        .await
        .unwrap();
    images::set_prepared(
        &pg,
        &untagged,
        51,
        &serde_json::json!({}),
        false,
        Some("v0.22.5"),
    )
    .await
    .unwrap();
    assert_eq!(
        images::record(&pg, &untagged)
            .await
            .unwrap()
            .unwrap()
            .version
            .as_deref(),
        Some("v0.22.5")
    );

    // Good, then bad: bad sticks — a later "good" never revives it.
    images::mark_good(&pg, &d).await.unwrap();
    let r = images::record(&pg, &d).await.unwrap().unwrap();
    assert_eq!(r.status, images::Status::Good);
    assert!(r.good_since.is_some());
    images::mark_bad(&pg, &d, "the canary did not report healthy")
        .await
        .unwrap();
    images::mark_good(&pg, &d).await.unwrap();
    let r = images::record(&pg, &d).await.unwrap().unwrap();
    assert_eq!(r.status, images::Status::Bad);
    assert_eq!(
        r.reason.as_deref(),
        Some("the canary did not report healthy")
    );
    assert_eq!(r.good_since, None);

    // The compiled pin's seed never overwrites a record.
    images::seed_known_good(&pg, &d, "x", "v0.0.1", 1, "seed")
        .await
        .unwrap();
    assert_eq!(
        images::record(&pg, &d).await.unwrap().unwrap().status,
        images::Status::Bad
    );

    // Pruned: no longer on the host, so not runnable until prepared again.
    images::note_pruned(&pg, &untagged).await.unwrap();
    let r = images::record(&pg, &untagged).await.unwrap().unwrap();
    assert!(!r.runnable() && !r.pulled_here);
    cleanup(&pg, tag).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_assignment_is_one_per_agent_and_leaves_with_the_agent() {
    let pg = pg().await;
    let tag = "asg";
    cleanup(&pg, tag).await;
    let (id, _) = agent(&pg, tag, "ada").await;
    let (d1, d2) = (digest(tag, 0), digest(tag, 1));
    images::assign(&pg, &id, &d1).await.unwrap();
    images::assign(&pg, &id, &d2).await.unwrap();
    assert_eq!(images::assignments(&pg).await.unwrap().get(&id), Some(&d2));
    sqlx::query("delete from agent_defs where id = $1::uuid")
        .bind(&id)
        .execute(&pg)
        .await
        .unwrap();
    assert_eq!(
        images::assignments(&pg).await.unwrap().get(&id),
        None,
        "cascaded"
    );
    cleanup(&pg, tag).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_agent_mid_reply_or_mid_conversation_is_busy_and_a_quiet_one_is_not() {
    let pg = pg().await;
    let tag = "busy";
    cleanup(&pg, tag).await;
    let (_, model) = agent(&pg, tag, "bo").await;
    let user: String = sqlx::query_scalar(
        "insert into users (sub, email, name, role) values ($1, $1, 'b', 'member') returning id::text",
    )
    .bind(format!("{PREFIX}:{tag}:u"))
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(busy(&pg, &model).await.unwrap(), None, "a quiet agent");

    let conv: String = sqlx::query_scalar(
        "insert into conversations (user_id, agent_model) values ($1::uuid, $2) returning id::text",
    )
    .bind(&user)
    .bind(&model)
    .fetch_one(&pg)
    .await
    .unwrap();
    // A reply being written an hour into its conversation: busy on the
    // stream, not on the conversation's age.
    let msg: String = sqlx::query_scalar(
        "insert into messages (conversation_id, seq, role, status, created_at, streamed_at) \
         values ($1::uuid, 1, 'assistant', 'streaming', now() - interval '1 hour', now()) \
         returning id::text",
    )
    .bind(&conv)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(
        busy(&pg, &model).await.unwrap().as_deref(),
        Some("it is writing a reply")
    );
    // Its writer gone past the lease: an orphan, not a turn.
    sqlx::query(
        "update messages set streamed_at = now() - interval '20 minutes' where id = $1::uuid",
    )
    .bind(&msg)
    .execute(&pg)
    .await
    .unwrap();
    assert_eq!(busy(&pg, &model).await.unwrap(), None);
    // A message a minute ago: someone is in a conversation with it.
    sqlx::query(
        "insert into messages (conversation_id, seq, role, created_at) \
         values ($1::uuid, 2, 'user', now() - interval '1 minute')",
    )
    .bind(&conv)
    .execute(&pg)
    .await
    .unwrap();
    assert!(
        busy(&pg, &model)
            .await
            .unwrap()
            .unwrap()
            .contains("a conversation with it")
    );
    sqlx::query("delete from conversations where id = $1::uuid")
        .bind(&conv)
        .execute(&pg)
        .await
        .unwrap();
    // A live work session.
    sqlx::query(
        "insert into runs (kind, state, input) values ('work-session', 'running', jsonb_build_object('agentModel', $1::text))",
    )
    .bind(&model)
    .execute(&pg)
    .await
    .unwrap();
    assert_eq!(
        busy(&pg, &model).await.unwrap().as_deref(),
        Some("it has a work session in progress")
    );
    sqlx::query("delete from runs where kind = 'work-session' and input->>'agentModel' = $1")
        .bind(&model)
        .execute(&pg)
        .await
        .unwrap();
    // A model call a minute ago (a cron, a delegated turn).
    sqlx::query(
        "insert into usage_events (agent_model, source, created_at) values ($1, 'unattended', now() - interval '1 minute')",
    )
    .bind(&model)
    .execute(&pg)
    .await
    .unwrap();
    assert!(
        busy(&pg, &model)
            .await
            .unwrap()
            .unwrap()
            .contains("called a model")
    );
    sqlx::query("delete from usage_events where agent_model = $1")
        .bind(&model)
        .execute(&pg)
        .await
        .unwrap();
    assert_eq!(busy(&pg, &model).await.unwrap(), None, "quiet again");
    cleanup(&pg, tag).await;
}
