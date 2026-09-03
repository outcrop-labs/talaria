// Live-DB proof of the agent-reply fan-out (cargo test -- --ignored). The
// feature is three gates stacked on one writer — the read cursor ("still
// looking" files nothing), the unread dedupe (one pointer per thread, further
// replies fold in), and the kind→href mapping (chat→thread, plan→plan,
// research→the RUN, so opening the run is the seen gesture) — and every one
// of them is a WHERE clause only Postgres can confirm. House rule:
// #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test --test agent_reply_notify -- --ignored

use sqlx::postgres::PgPool;
use talaria_api::conversations::{create_conversation, mark_conversation_read};
use talaria_api::notify::{NotifyDeps, notify_agent_reply, notify_class_of};

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

/// Two throwaway people — a thread owner and a plan collaborator. The
/// cascade takes conversations, members, reads, messages, notifications, and
/// research_runs (owner_user_id) with them.
async fn cleanup(pg: &PgPool) {
    sqlx::query("delete from users where email like 'agent-reply-%@test.invalid'")
        .execute(pg)
        .await
        .unwrap();
}

async fn person(pg: &PgPool, sub: &str, email: &str, name: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, $3, 'member') returning id::text",
    )
    .bind(sub)
    .bind(email)
    .bind(name)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// Land an assistant turn and return its message id — direct rows, because
/// this suite proves the fan-out's gates, not the streaming machinery.
async fn reply(pg: &PgPool, conversation: &str, seq: i32, content: &str, status: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into messages (conversation_id, seq, role, status, content) \
         values ($1::uuid, $2, 'assistant', $3, $4) returning id::text",
    )
    .bind(conversation)
    .bind(seq)
    .bind(status)
    .bind(content)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// The user's unread agent-reply rows for one href.
async fn unread_rows(pg: &PgPool, user: &str, kind: &str, href: &str) -> Vec<(String, String)> {
    sqlx::query_as(
        "select title, body from notifications \
         where user_id = $1::uuid and kind = $2 and href = $3 and read_at is null",
    )
    .bind(user)
    .bind(kind)
    .bind(href)
    .fetch_all(pg)
    .await
    .unwrap()
}

/// The local spelling of the crate's pub(crate) counter — the contract this
/// suite pins is the NUMBER, not the helper.
fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_reply_rings_once_for_whoever_was_away() {
    let pg = pool().await;
    cleanup(&pg).await;
    let deps = NotifyDeps::publishing(pg.clone(), None);
    let owner = person(
        &pg,
        "agent-reply-owner",
        "agent-reply-owner@test.invalid",
        "Reply Owner",
    )
    .await;
    let mate = person(
        &pg,
        "agent-reply-mate",
        "agent-reply-mate@test.invalid",
        "Reply Mate",
    )
    .await;

    // — The owner walked away from their agent chat mid-reply.
    let chat = create_conversation(&pg, &owner, "claude-test", "Away chat", "chat", None)
        .await
        .unwrap();
    let href = format!("/comms/agent/claude-test/{chat}");
    let first = reply(&pg, &chat, 1, "the answer, at length", "complete").await;
    notify_agent_reply(&deps, &chat, &first).await;
    let rows = unread_rows(&pg, &owner, "agent-reply", &href).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "Claude replied"); // first dash splits the label
    // The dm class — toasts and mail ride it, exactly like a DM.
    assert_eq!(notify_class_of("agent-reply"), "dm");

    // A second reply while the first row still sits unread FOLDS IN: one
    // pointer per thread, not a pile from a fast back-and-forth.
    let second = reply(&pg, &chat, 2, "one more thing", "complete").await;
    notify_agent_reply(&deps, &chat, &second).await;
    assert_eq!(
        unread_rows(&pg, &owner, "agent-reply", &href).await.len(),
        1
    );

    // The owner comes back: reads to latest AND clicks the bell row away.
    // The cursor alone now files nothing for the covered reply — no dedupe
    // needed to help it.
    sqlx::query(
        "update notifications set read_at = now() \
                 where user_id = $1::uuid and kind = 'agent-reply' and href = $2",
    )
    .bind(&owner)
    .bind(&href)
    .execute(&pg)
    .await
    .unwrap();
    mark_conversation_read(&pg, &chat, &owner, 2).await.unwrap();
    notify_agent_reply(&deps, &chat, &second).await;
    assert_eq!(
        unread_rows(&pg, &owner, "agent-reply", &href).await.len(),
        0
    );

    // Walking away again re-arms: the next reply rings once more.
    let while_reading = reply(&pg, &chat, 3, "said after you looked away", "complete").await;
    notify_agent_reply(&deps, &chat, &while_reading).await;
    assert_eq!(
        unread_rows(&pg, &owner, "agent-reply", &href).await.len(),
        1
    );

    // Replies that never landed ring nobody: still streaming, or complete
    // but empty.
    let streaming = reply(&pg, &chat, 4, "half a thought", "streaming").await;
    notify_agent_reply(&deps, &chat, &streaming).await;
    let empty = reply(&pg, &chat, 5, "   ", "complete").await;
    notify_agent_reply(&deps, &chat, &empty).await;
    assert_eq!(
        unread_rows(&pg, &owner, "agent-reply", &href).await.len(),
        1
    );

    // — A shared plan: BOTH the owner and the collaborator are away, both
    // cursors behind, so both get the one row — with the plan's href.
    let plan = create_conversation(&pg, &owner, "claude-test", "Shared plan", "plan", None)
        .await
        .unwrap();
    sqlx::query(
        "insert into conversation_members (conversation_id, user_id) values ($1::uuid, $2::uuid)",
    )
    .bind(&plan)
    .bind(&mate)
    .execute(&pg)
    .await
    .unwrap();
    let plan_reply = reply(&pg, &plan, 0, "step two, decided", "complete").await;
    notify_agent_reply(&deps, &plan, &plan_reply).await;
    let plan_href = format!("/plan/{plan}");
    assert_eq!(
        unread_rows(&pg, &owner, "agent-reply", &plan_href)
            .await
            .len(),
        1
    );
    assert_eq!(
        unread_rows(&pg, &mate, "agent-reply", &plan_href)
            .await
            .len(),
        1
    );
    // The mate reads to latest; the next reply is behind their cursor again
    // and would ring — but the unread pointer folds it, so their row still
    // says exactly 1. The bell clears it when they click.
    mark_conversation_read(&pg, &plan, &mate, 0).await.unwrap();
    let later = reply(&pg, &plan, 1, "a revision", "complete").await;
    notify_agent_reply(&deps, &plan, &later).await;
    assert_eq!(
        unread_rows(&pg, &mate, "agent-reply", &plan_href)
            .await
            .len(),
        1
    );

    // — A research discussion points at the RUN: opening the run marks this
    // href read, so the bell row and the rail badge clear in one click.
    let run: (String,) = sqlx::query_as(
        "insert into research_runs (requested_by, agent_model, mode, question, status) \
         values ('Reply Owner', 'claude-test', 'brief', 'agent-reply suite?', 'done') \
         returning id::text",
    )
    .fetch_one(&pg)
    .await
    .unwrap();
    let research = create_conversation(&pg, &owner, "claude-test", "Run talk", "research", None)
        .await
        .unwrap();
    sqlx::query("update research_runs set conversation_id = $2::uuid where id = $1::uuid")
        .bind(&run.0)
        .bind(&research)
        .execute(&pg)
        .await
        .unwrap();
    let research_reply = reply(&pg, &research, 0, "about the second source", "complete").await;
    notify_agent_reply(&deps, &research, &research_reply).await;
    assert_eq!(
        unread_rows(&pg, &owner, "agent-reply", &format!("/research/{}", run.0))
            .await
            .len(),
        1
    );

    // — The truncation contract, same as a DM body: 200 UTF-16 units, '…'
    // only past the bound, and never a broken half-pair.
    let long_chat = create_conversation(&pg, &owner, "claude-test", "Long chat", "chat", None)
        .await
        .unwrap();
    let long_href = format!("/comms/agent/claude-test/{long_chat}");
    let long = "😀".repeat(101); // astral: 2 UTF-16 units each — 202 > 200
    let long_reply = reply(&pg, &long_chat, 0, &long, "complete").await;
    notify_agent_reply(&deps, &long_chat, &long_reply).await;
    let rows = unread_rows(&pg, &owner, "agent-reply", &long_href).await;
    assert_eq!(rows.len(), 1);
    // 100 whole emoji (200 units) + the ellipsis — a lone surrogate never
    // escaped the cut.
    assert_eq!(utf16_len(&rows[0].1), 201);
    assert!(rows[0].1.ends_with('…'));

    cleanup(&pg).await;
}
