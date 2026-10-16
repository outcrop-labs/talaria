// Live-DB proof of WHO CAN SEE WHICH CONVERSATION, per kind.
//
//   DATABASE_URL=postgres://… cargo test --test it conversation_audience_live:: -- --ignored
//
// WHY THIS NEEDS POSTGRES. The whole feature is one WHERE clause with three
// audience arms, two of them reaching through join tables, and the predicate
// is only true or false once Postgres evaluates it. A unit test over a fake
// would be a test of the fake.
//
// WHAT IT IS GUARDING, specifically. Listing a SET of kinds made two things
// reachable that one kind per call had kept theoretical:
//
//   1. The membership arm tested `$2 in ('plan','work')` — the kinds the
//      CALLER ASKED FOR — rather than the row's own kind, so it gated whether
//      membership is consulted at all on the REQUEST instead of on the row.
//      The exists-subquery is still keyed on `c.id`, so this was never an
//      org-wide leak; what it means is that a chat carrying a
//      `conversation_members` row — which nothing in the schema prevents —
//      lists as soon as the same call also asks for plans. The
//      `lists_no_chat_with_a_stray_membership_row` case below plants exactly
//      that and fails against the old form.
//
//   2. `research` was absent from the route's whitelist, so asking for it
//      returned CHATS. A research thread's audience is its RUN's audience —
//      owner, member, or a team the run was shared with — because a thread
//      about a report must not be readable by someone who cannot read the
//      report.

use crate::support::{person, pg};
use sqlx::postgres::PgPool;
use talaria_api::conversations::{create_conversation, list_conversations_in};

const TAG: &str = "conv-audience";

/// PER-TEST CLEANUP PREFIXES, not one for the file. The modules of this binary
/// run their tests in PARALLEL threads against one scratch database, so a
/// sweep keyed on a prefix both tests share deletes the other test's users
/// out from under it — which fails as a foreign-key violation on the next
/// insert and reads like a schema problem rather than a test-isolation one.
async fn cleanup(pg: &PgPool, slot: &str) {
    sqlx::query("delete from users where email like $1")
        .bind(format!("conv-audience-{slot}-%@test.invalid"))
        .execute(pg)
        .await
        .unwrap();
}

/// A research run pointing at a conversation, owned by `owner`.
async fn run_for(pg: &PgPool, conversation: &str, owner: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into research_runs \
           (conversation_id, owner_user_id, question, mode, agent_model, requested_by) \
         values ($1::uuid, $2::uuid, 'what did we ship', 'brief', 'a', 'x') \
         returning id::text",
    )
    .bind(conversation)
    .bind(owner)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

async fn titles(pg: &PgPool, user: &str, kinds: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = list_conversations_in(pg, user, kinds, false)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|r| r.title)
        .collect();
    out.sort();
    out
}

#[tokio::test]
#[ignore]
async fn lists_by_each_kinds_own_audience_rule() {
    let pg = pg().await;
    cleanup(&pg, "rules").await;

    let me = person(
        &pg,
        &format!("{TAG}:rules:me"),
        "conv-audience-rules-me@test.invalid",
        "Me",
    )
    .await;
    let them = person(
        &pg,
        &format!("{TAG}:rules:them"),
        "conv-audience-rules-them@test.invalid",
        "Them",
    )
    .await;

    // Mine, one per kind.
    for kind in ["chat", "plan", "work", "research"] {
        create_conversation(&pg, &me, "a", &format!("mine {kind}"), kind, None)
            .await
            .unwrap();
    }

    // Theirs, shared with me by MEMBERSHIP.
    let shared_plan = create_conversation(&pg, &them, "a", "shared plan", "plan", None)
        .await
        .unwrap();
    sqlx::query(
        "insert into conversation_members (conversation_id, user_id) values ($1::uuid, $2::uuid)",
    )
    .bind(&shared_plan)
    .bind(&me)
    .execute(&pg)
    .await
    .unwrap();

    // Theirs, a research thread whose RUN I am a member of.
    let shared_research = create_conversation(&pg, &them, "a", "shared research", "research", None)
        .await
        .unwrap();
    let run = run_for(&pg, &shared_research, &them).await;
    sqlx::query("insert into research_members (run_id, user_id) values ($1::uuid, $2::uuid)")
        .bind(&run)
        .bind(&me)
        .execute(&pg)
        .await
        .unwrap();

    // Theirs, with no membership anywhere. Must never list for me.
    create_conversation(&pg, &them, "a", "not mine chat", "chat", None)
        .await
        .unwrap();
    let hidden_research =
        create_conversation(&pg, &them, "a", "not mine research", "research", None)
            .await
            .unwrap();
    run_for(&pg, &hidden_research, &them).await;

    // ── chats are strictly your own ──────────────────────────────────────
    assert_eq!(titles(&pg, &me, &["chat"]).await, ["mine chat"]);

    // ── plans and work sessions are multiplayer ──────────────────────────
    assert_eq!(
        titles(&pg, &me, &["plan"]).await,
        ["mine plan", "shared plan"]
    );
    assert_eq!(titles(&pg, &me, &["work"]).await, ["mine work"]);

    // ── a research thread follows its RUN's audience ─────────────────────
    // This is the case that used to return chats, because `research` was not
    // in the route's whitelist at all.
    assert_eq!(
        titles(&pg, &me, &["research"]).await,
        ["mine research", "shared research"]
    );

    // ── the merged surface asks for all three at once ────────────────────
    assert_eq!(
        titles(&pg, &me, &["work", "plan", "research"]).await,
        [
            "mine plan",
            "mine research",
            "mine work",
            "shared plan",
            "shared research"
        ]
    );

    cleanup(&pg, "rules").await;
}

#[tokio::test]
#[ignore]
async fn lists_no_chat_with_a_stray_membership_row() {
    // THE DIFFERENCE A SET MADE REACHABLE. The old arm asked "did the caller
    // request plans or work" rather than "is this ROW a plan or a work
    // session". The membership subquery is keyed on the row either way, so
    // this was never an org-wide leak — but a chat that carries a membership
    // row listed as soon as the same call also asked for plans, and nothing
    // in the schema stops such a row existing. Testing the row's own kind is
    // the predicate that cannot be surprised by one.
    let pg = pg().await;
    cleanup(&pg, "stray").await;

    let me = person(
        &pg,
        &format!("{TAG}:stray:me"),
        "conv-audience-stray-me@test.invalid",
        "Me",
    )
    .await;
    let them = person(
        &pg,
        &format!("{TAG}:stray:them"),
        "conv-audience-stray-them@test.invalid",
        "Them",
    )
    .await;

    // Their plan, properly shared with me.
    let shared_plan = create_conversation(&pg, &them, "a", "their plan", "plan", None)
        .await
        .unwrap();
    // Their CHAT, carrying a membership row it has no business having.
    let stray_chat = create_conversation(&pg, &them, "a", "their chat", "chat", None)
        .await
        .unwrap();
    for id in [&shared_plan, &stray_chat] {
        sqlx::query(
            "insert into conversation_members (conversation_id, user_id) values ($1::uuid, $2::uuid)",
        )
        .bind(id)
        .bind(&me)
        .execute(&pg)
        .await
        .unwrap();
    }

    // Asked for alone, a chat is strictly its owner's whatever rows point at it.
    assert!(
        titles(&pg, &me, &["chat"]).await.is_empty(),
        "a chat is its owner's, membership row or not"
    );
    // And asking for plans in the SAME call must not change that answer.
    assert_eq!(
        titles(&pg, &me, &["chat", "plan"]).await,
        ["their plan"],
        "asking for plans alongside chats must not widen what a chat row means"
    );

    cleanup(&pg, "stray").await;
}
