// Live-DB proof of the chip-landing tiers (cargo test -- --ignored). The
// tiers are SQL: which channel_messages or messages row a fleet agent's
// tool-event chips land on, and whether they are dropped when no tier hits.
// None of it is provable without Postgres evaluating the interval
// arithmetic and the unique (channel_id, seq) the way it does. House rule:
// #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it chips_landing_live:: -- --ignored

use crate::support::{fabricate_user, pg, sweep_user_rows};
use sqlx::PgPool;
use talaria_chips::{chips_from_tool, surface_tool_chips};

/// The whole fixture hangs off one fabricated user and one channel, and the
/// deletes run in dependency order: the channel first — channel_messages
/// cascade off it, and so does the chat_approvals row a test may have landed
/// (its channel leg is ON DELETE CASCADE) — then the user. The channel row
/// itself survives its creator (created_by is SET NULL), so it goes
/// explicitly. Every test hangs off its OWN tag — tests run concurrently,
/// and one test's reset deleting a sibling's channel mid-flight is the
/// failure this layout exists to prevent.
async fn reset(pg: &PgPool, tag: &str) {
    sqlx::query("delete from channels where name like $1")
        .bind(format!("chips-landing-{tag}-%"))
        .execute(pg)
        .await
        .unwrap();
    sweep_user_rows(pg, "chips-landing", tag).await;
}

/// One user, one channel, one completed channel message authored by the
/// fake agent model at `age_minutes`. The row is what production writes when
/// a channel turn finishes before the plugin's tool-event POST arrives —
/// the exact race TALA-86 is about.
async fn fixture(pg: &PgPool, tag: &str, age_minutes: i64) -> (String, String, String) {
    reset(pg, tag).await;
    let user = fabricate_user(pg, "chips-landing", tag).await;
    let (channel,): (String,) = sqlx::query_as(
        "insert into channels (name, created_by) values ($1, $2::uuid) returning id::text",
    )
    .bind(format!("chips-landing-{tag}-room"))
    .bind(&user)
    .fetch_one(pg)
    .await
    .unwrap();
    let agent = format!("chips-agent-{tag}");
    let (message,): (String,) = sqlx::query_as(
        "insert into channel_messages (channel_id, seq, author_type, author, status, created_at) \
         values ($1::uuid, 1, 'agent', $2, 'complete', now() - ($3::bigint * interval '1 minute')) \
         returning id::text",
    )
    .bind(&channel)
    .bind(&agent)
    .bind(age_minutes)
    .fetch_one(pg)
    .await
    .unwrap();
    (agent, channel, message)
}

/// The chips column as stored on a channel message row — the landed truth,
/// read back straight from Postgres rather than from the call's return.
async fn chips_on(pg: &PgPool, message: &str) -> serde_json::Value {
    let (chips,): (serde_json::Value,) =
        sqlx::query_as("select chips from channel_messages where id = $1::uuid")
            .bind(message)
            .fetch_one(pg)
            .await
            .unwrap();
    chips
}

/// A completed (non-streaming) channel turn is still the agent's newest —
/// the bounded fallback tier TALA-86 adds. Without it, a turn that finishes
/// before the plugin's tool-event POST arrives loses every chip it created,
/// silently.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn fast_completed_channel_turn_still_gets_its_link_chip() {
    let pg = pg().await;
    let tag = "fresh";
    let (agent, channel, message) = fixture(&pg, tag, 0).await;
    let chips = chips_from_tool(
        "create_document",
        r#"{"title":"Notes"}"#,
        r#"{"id":"7e2a1d90-5c34-4b6f-8a11-2d4e9f0b3c77","title":"Notes"}"#,
    );
    let landed = surface_tool_chips(&pg, None, &agent, None, chips)
        .await
        .unwrap()
        .expect("a fresh completed turn is a landing target");
    assert_eq!(landed.channel_id.as_deref(), Some(channel.as_str()));
    assert_eq!(landed.message_id, message);
    let chips = chips_on(&pg, &message).await;
    let arr = chips.as_array().expect("chips is an array");
    assert_eq!(arr.len(), 1, "exactly the one link chip: {chips}");
    assert_eq!(arr[0]["kind"], "link");
    assert_eq!(
        arr[0]["href"],
        "/artifacts?a=7e2a1d90-5c34-4b6f-8a11-2d4e9f0b3c77"
    );
    reset(&pg, tag).await;
}

/// Past the 15-minute bound the same completed row is no longer a target:
/// the agent has no live turn, and the chips are dropped rather than landed
/// on a stale one.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn stale_fallback_expires() {
    let pg = pg().await;
    let tag = "stale";
    let (agent, _channel, message) = fixture(&pg, tag, 30).await;
    let chips = chips_from_tool(
        "create_document",
        r#"{"title":"Notes"}"#,
        r#"{"id":"7e2a1d90-5c34-4b6f-8a11-2d4e9f0b3c77","title":"Notes"}"#,
    );
    let landed = surface_tool_chips(&pg, None, &agent, None, chips)
        .await
        .unwrap();
    assert!(
        landed.is_none(),
        "a stale turn must not be a landing target"
    );
    let chips = chips_on(&pg, &message).await;
    assert!(
        chips.as_array().is_none_or(|a| a.is_empty()),
        "a dropped chip must not be written: {chips}"
    );
    reset(&pg, tag).await;
}

/// An approval chip does not carry the plugin's pending id to the surface —
/// surface_tool_chips rewrites actionId to the chat_approvals row's id, so
/// the decide endpoint is one door. The external id stays on the row as the
/// dedupe key for a retried tool call.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn approval_chip_lands_with_action_id_rewritten() {
    let pg = pg().await;
    let tag = "approval";
    let (agent, channel, message) = fixture(&pg, tag, 0).await;
    let chips = chips_from_tool(
        "draft_email",
        r#"{"to":"a@b.c"}"#,
        r#"{"pending":{"id":"p-x"},"message":"Drafted — waiting"}"#,
    );
    let landed = surface_tool_chips(&pg, None, &agent, None, chips)
        .await
        .unwrap()
        .expect("a fresh completed turn is a landing target");
    assert_eq!(landed.message_id, message);
    let (row_id, external): (String, Option<String>) = sqlx::query_as(
        "select id::text, external_id from chat_approvals \
         where agent_model = $1 and external_id = 'p-x'",
    )
    .bind(&agent)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(external.as_deref(), Some("p-x"));
    let (approval_channel,): (Option<String>,) =
        sqlx::query_as("select channel_id::text from chat_approvals where id = $1::uuid")
            .bind(&row_id)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(approval_channel.as_deref(), Some(channel.as_str()));
    let chips = chips_on(&pg, &message).await;
    let arr = chips.as_array().expect("chips is an array");
    assert_eq!(arr.len(), 1, "exactly the one approval chip: {chips}");
    assert_eq!(arr[0]["kind"], "approval");
    assert_eq!(
        arr[0]["actionId"], row_id,
        "rewritten to the chat_approvals row, not the plugin's pending id"
    );
    assert_ne!(arr[0]["actionId"], "p-x");
    reset(&pg, tag).await;
}
