// Live-DB proof for the pending-draft dedupe (cargo test -- --ignored).
// The report this file pins, 2026-09-11: the dogfood brief held six
// SEND EMAIL cards for one reply, because a retried agent run re-drafts
// the same email — reworded body, reformatted recipients — and every
// draft became its own p0 row. Approving two of them was a double-send.
// The fix is that queue_action returns the already-pending row instead of
// inserting a sixth, matched by principal + recipients + subject with the
// formatting boiled off, and these assertions drive the real queue twice
// against the real table.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test --test it pending_dedupe_live:: -- --ignored

use crate::support::{app_state, fabricate_user, pg};
use serde_json::{Value, json};
use talaria_api::google::pending_actions::{QueueAction, queue_action};
use talaria_api::realtime::RealtimeDeps;
/// Redis on a dead port, exactly like the other live tests: the announce
/// fan-out degrades to a no-op, and this file is about the rows.
async fn cleanup(pg: &sqlx::PgPool, tag: &str) {
    let pattern = format!("pending-dedupe:{tag}:%");
    sqlx::query(
        "delete from google_pending_actions where owner_user_id in \
         (select id from users where sub like $1)",
    )
    .bind(&pattern)
    .execute(pg)
    .await
    .unwrap();
    sqlx::query(
        "delete from notifications where user_id in (select id from users where sub like $1)",
    )
    .bind(&pattern)
    .execute(pg)
    .await
    .unwrap();
    // The detached announce's marks in app_settings are keyed by the (now
    // deleted) action ids and pruned by the sweep's own prune_announced —
    // shared state this cleanup must not wipe.
    sqlx::query("delete from users where sub like $1")
        .bind(&pattern)
        .execute(pg)
        .await
        .unwrap();
}

/// QueueAction borrows its input, and the queue stores the payload as-is;
/// leaking the fixture keeps the borrow honest for a one-shot test.
fn draft(owner: &str, payload: Value) -> QueueAction<'static> {
    QueueAction {
        kind: "gmail_send",
        summary: "test draft",
        payload: Box::leak(payload.into()),
        agent_model: "test-agent",
        owner_user_id: Some(Box::leak(owner.to_string().into_boxed_str())),
        is_org: false,
        principal_kind: "owner",
    }
}

#[tokio::test]
#[ignore]
async fn a_redrafted_email_converges_on_the_pending_row() {
    let pg = pg().await;
    let state = app_state().await;
    let tag = "converge";
    cleanup(&pg, tag).await;
    let owner = fabricate_user(&pg, "pending-dedupe", tag).await;

    let first = queue_action(
        &state.pg,
        RealtimeDeps::publish_only(None),
        &draft(
            &owner,
            json!({
                "to": "Gareth Lynch <Gareth@x.com>, Toby@x.com",
                "subject": "Re: Proposal & Deck",
                "body": "First wording."
            }),
        ),
    )
    .await
    .expect("first queue");
    assert!(!first.already_pending);

    // The re-draft: different body wording, reformatted recipients — the
    // same ask.
    let redraft = queue_action(
        &state.pg,
        RealtimeDeps::publish_only(None),
        &draft(
            &owner,
            json!({
                "to": "gareth@x.com, toby@x.com",
                "subject": "Re:  Proposal & Deck ",
                "body": "A DIFFERENT wording."
            }),
        ),
    )
    .await
    .expect("second queue");
    assert!(redraft.already_pending);
    assert_eq!(redraft.action.id, first.action.id);

    let count: i64 = sqlx::query_scalar(
        "select count(*) from google_pending_actions where owner_user_id = $1::uuid",
    )
    .bind(&owner)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(count, 1, "the re-draft must not insert a second row");

    cleanup(&pg, tag).await;
}

#[tokio::test]
#[ignore]
async fn a_decided_row_is_a_fresh_ask_again() {
    let pg = pg().await;
    let state = app_state().await;
    let tag = "freshask";
    cleanup(&pg, tag).await;
    let owner = fabricate_user(&pg, "pending-dedupe", tag).await;

    let queued = queue_action(
        &state.pg,
        RealtimeDeps::publish_only(None),
        &draft(
            &owner,
            json!({"to": "a@x.com", "subject": "Hi", "body": "b"}),
        ),
    )
    .await
    .expect("first queue");

    sqlx::query("update google_pending_actions set status = 'rejected' where id = $1::uuid")
        .bind(&queued.action.id)
        .execute(&pg)
        .await
        .unwrap();

    let requeued = queue_action(
        &state.pg,
        RealtimeDeps::publish_only(None),
        &draft(
            &owner,
            json!({"to": "a@x.com", "subject": "Hi", "body": "b"}),
        ),
    )
    .await
    .expect("requeue after reject");
    assert!(!requeued.already_pending, "a rejected draft is a fresh ask");
    assert_ne!(requeued.action.id, queued.action.id);

    cleanup(&pg, tag).await;
}

#[tokio::test]
#[ignore]
async fn a_different_subject_is_a_different_ask() {
    let pg = pg().await;
    let state = app_state().await;
    let tag = "diffsubj";
    cleanup(&pg, tag).await;
    let owner = fabricate_user(&pg, "pending-dedupe", tag).await;

    queue_action(
        &state.pg,
        RealtimeDeps::publish_only(None),
        &draft(
            &owner,
            json!({"to": "a@x.com", "subject": "Hi", "body": "b"}),
        ),
    )
    .await
    .expect("first queue");

    let other = queue_action(
        &state.pg,
        RealtimeDeps::publish_only(None),
        &draft(
            &owner,
            json!({"to": "a@x.com", "subject": "Bye", "body": "b"}),
        ),
    )
    .await
    .expect("second queue");
    assert!(!other.already_pending);

    cleanup(&pg, tag).await;
}
