// Live-DB proof that a conversation event reaches everyone the read gates
// admit and nobody else: a team the plan was shared with, a research run's
// collaborator, and never a chat's stray member. The audience is a WHERE
// clause only Postgres can confirm, so these are `#[ignore]`d and CI's
// live-tests job runs them (`cargo test --workspace --lib -- --ignored`):
//
//   DATABASE_URL=postgres://… cargo test -p talaria-conversations --lib -- --ignored audience_live
//
// The publish edge is captured, not Redis: the question is routing. Each test
// owns its own name prefix, because the tests run in parallel on one database.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::PgPool;
use talaria_notify::NotifyDeps;
use talaria_realtime::RealtimeDeps;

use super::orphan_live::pg;
use super::*;

async fn cleanup(pg: &PgPool, slot: &str) {
    sqlx::query("delete from teams where name = $1")
        .bind(format!("audience-live-{slot}"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from users where email like $1")
        .bind(format!("audience-live-{slot}-%@test.invalid"))
        .execute(pg)
        .await
        .unwrap();
}

async fn reader(pg: &PgPool, slot: &str, who: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, $1, 'member') \
         returning id::text",
    )
    .bind(format!("audience-live-{slot}-{who}"))
    .bind(format!("audience-live-{slot}-{who}@test.invalid"))
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

async fn conversation(pg: &PgPool, owner: &str, kind: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into conversations (user_id, agent_model, kind) \
         values ($1::uuid, 'claude-test', $2) returning id::text",
    )
    .bind(owner)
    .bind(kind)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// Fan once through the real `fan_conversation_event` and return the sorted
/// `user:<id>` topics it published, settled on `expected`. The fan publishes
/// its whole audience in one synchronous loop, so a wider audience than
/// expected is already captured by the time the count is reached.
async fn fanned(pg: &PgPool, conversation_id: &str, expected: usize) -> Vec<String> {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = seen.clone();
    let id_only = format!(r#"{{"type":"conversation","conversationId":"{conversation_id}"}}"#);
    let realtime = RealtimeDeps {
        publish: Arc::new(move |topic: &str, payload: &str| {
            assert_eq!(payload, id_only, "the event is id-only");
            sink.lock().unwrap().push(topic.to_string());
        }),
        subscribe: Arc::new(|_| Box::pin(async { tokio::sync::mpsc::channel::<String>(1).1 })),
    };
    fan_conversation_event(
        NotifyDeps {
            pg: pg.clone(),
            realtime,
        },
        conversation_id.to_string(),
    );
    for _ in 0..200 {
        if seen.lock().unwrap().len() >= expected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut topics = seen.lock().unwrap().clone();
    topics.sort();
    topics
}

fn users(ids: &[&String]) -> Vec<String> {
    let mut out: Vec<String> = ids.iter().map(|id| format!("user:{id}")).collect();
    out.sort();
    out
}

#[tokio::test]
#[ignore]
async fn audience_live_a_team_shared_plan_reaches_its_team() {
    let pg = pg().await;
    cleanup(&pg, "team").await;
    let owner = reader(&pg, "team", "owner").await;
    let teammate = reader(&pg, "team", "mate").await;
    reader(&pg, "team", "outsider").await;

    let (team,): (String,) =
        sqlx::query_as("insert into teams (name) values ('audience-live-team') returning id::text")
            .fetch_one(&pg)
            .await
            .unwrap();
    sqlx::query(
        "insert into team_members (team_id, user_id, role) values ($1::uuid, $2::uuid, 'member')",
    )
    .bind(&team)
    .bind(&teammate)
    .execute(&pg)
    .await
    .unwrap();
    let plan = conversation(&pg, &owner, "plan").await;
    sqlx::query(
        "insert into conversation_teams (conversation_id, team_id) values ($1::uuid, $2::uuid)",
    )
    .bind(&plan)
    .bind(&team)
    .execute(&pg)
    .await
    .unwrap();

    assert_eq!(
        fanned(&pg, &plan, 2).await,
        users(&[&owner, &teammate]),
        "a plan shared only through a team reaches the team, and the outsider never appears"
    );
    cleanup(&pg, "team").await;
}

#[tokio::test]
#[ignore]
async fn audience_live_a_research_thread_reaches_its_runs_collaborator() {
    let pg = pg().await;
    cleanup(&pg, "research").await;
    let owner = reader(&pg, "research", "owner").await;
    let collaborator = reader(&pg, "research", "collaborator").await;
    reader(&pg, "research", "outsider").await;

    let research = conversation(&pg, &owner, "research").await;
    let (run,): (String,) = sqlx::query_as(
        "insert into research_runs \
           (conversation_id, owner_user_id, question, mode, agent_model, requested_by) \
         values ($1::uuid, $2::uuid, 'what did we ship', 'brief', 'claude-test', 'x') \
         returning id::text",
    )
    .bind(&research)
    .bind(&owner)
    .fetch_one(&pg)
    .await
    .unwrap();
    sqlx::query("insert into research_members (run_id, user_id) values ($1::uuid, $2::uuid)")
        .bind(&run)
        .bind(&collaborator)
        .execute(&pg)
        .await
        .unwrap();

    assert_eq!(
        fanned(&pg, &research, 2).await,
        users(&[&owner, &collaborator]),
        "a research thread reaches its run's collaborator, and the outsider never appears"
    );
    cleanup(&pg, "research").await;
}

#[tokio::test]
#[ignore]
async fn audience_live_a_chat_reaches_its_owner_alone() {
    let pg = pg().await;
    cleanup(&pg, "chat").await;
    let owner = reader(&pg, "chat", "owner").await;
    let stray = reader(&pg, "chat", "stray").await;

    let chat = conversation(&pg, &owner, "chat").await;
    sqlx::query(
        "insert into conversation_members (conversation_id, user_id) values ($1::uuid, $2::uuid)",
    )
    .bind(&chat)
    .bind(&stray)
    .execute(&pg)
    .await
    .unwrap();

    assert_eq!(
        fanned(&pg, &chat, 1).await,
        users(&[&owner]),
        "a chat's membership row admits no reader, so it rings no one but the owner"
    );
    cleanup(&pg, "chat").await;
}

#[tokio::test]
#[ignore]
async fn audience_live_the_gate_and_the_fan_agree_on_every_path() {
    let pg = pg().await;
    cleanup(&pg, "agree").await;
    let mut people = Vec::new();
    for who in [
        "owner",
        "member",
        "plan-team",
        "run-owner",
        "run-member",
        "run-team",
        "stray",
        "outsider",
    ] {
        people.push(reader(&pg, "agree", who).await);
    }
    let [
        owner,
        member,
        plan_team,
        run_owner,
        run_member,
        run_team,
        stray,
        _outsider,
    ] = people.clone().try_into().unwrap();

    let (team,): (String,) = sqlx::query_as(
        "insert into teams (name) values ('audience-live-agree') returning id::text",
    )
    .fetch_one(&pg)
    .await
    .unwrap();
    for who in [&plan_team, &run_team] {
        sqlx::query(
            "insert into team_members (team_id, user_id, role) values ($1::uuid, $2::uuid, 'member')",
        )
        .bind(&team)
        .bind(who)
        .execute(&pg)
        .await
        .unwrap();
    }
    let mut conversations = Vec::new();
    for kind in ["chat", "plan", "work", "research"] {
        let id = conversation(&pg, &owner, kind).await;
        sqlx::query(
            "insert into conversation_members (conversation_id, user_id) values ($1::uuid, $2::uuid)",
        )
        .bind(&id)
        .bind(if kind == "chat" { &stray } else { &member })
        .execute(&pg)
        .await
        .unwrap();
        if kind == "plan" || kind == "work" {
            sqlx::query(
                "insert into conversation_teams (conversation_id, team_id) values ($1::uuid, $2::uuid)",
            )
            .bind(&id)
            .bind(&team)
            .execute(&pg)
            .await
            .unwrap();
        }
        if kind == "research" {
            let (run,): (String,) = sqlx::query_as(
                "insert into research_runs \
                   (conversation_id, owner_user_id, question, mode, agent_model, requested_by) \
                 values ($1::uuid, $2::uuid, 'q', 'brief', 'claude-test', 'x') returning id::text",
            )
            .bind(&id)
            .bind(&run_owner)
            .fetch_one(&pg)
            .await
            .unwrap();
            sqlx::query(
                "insert into research_members (run_id, user_id) values ($1::uuid, $2::uuid)",
            )
            .bind(&run)
            .bind(&run_member)
            .execute(&pg)
            .await
            .unwrap();
            sqlx::query("insert into research_teams (run_id, team_id) values ($1::uuid, $2::uuid)")
                .bind(&run)
                .bind(&team)
                .execute(&pg)
                .await
                .unwrap();
        }
        conversations.push((kind, id));
    }

    for (kind, id) in &conversations {
        let audience = conversation_audience_ids(&pg, id).await.unwrap();
        for person in &people {
            assert_eq!(
                conversation_accessible(&pg, person, id).await.unwrap(),
                audience.contains(person),
                "{kind}: the gate and the fan disagree about {person}"
            );
        }
    }
    let readers = |kind: &str| {
        let id = &conversations.iter().find(|(k, _)| *k == kind).unwrap().1;
        let pg = pg.clone();
        let id = id.clone();
        async move {
            let mut a = conversation_audience_ids(&pg, &id).await.unwrap();
            a.sort();
            a
        }
    };
    let sorted = |mut v: Vec<&String>| {
        v.sort();
        v.into_iter().cloned().collect::<Vec<String>>()
    };
    assert_eq!(readers("chat").await, sorted(vec![&owner]));
    assert_eq!(
        readers("plan").await,
        sorted(vec![&owner, &member, &plan_team, &run_team])
    );
    assert_eq!(
        readers("research").await,
        sorted(vec![&owner, &run_owner, &run_member, &plan_team, &run_team])
    );
    cleanup(&pg, "agree").await;
}
