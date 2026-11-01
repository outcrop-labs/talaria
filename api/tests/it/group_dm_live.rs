// Live-DB proof for group DMs — POST /api/dms { userIds, agents?, name? } and
// how GET /api/channels lists them. Proven through the real router against the
// real tables: the find-or-create on the whole participant set, the optional
// name, the agents' seats, the one-row listing with `members` and `agents`
// (and no single `peer`), and the refusals.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it group_dm_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "group-dm-live";

async fn member(state: &AppState) -> (String, String) {
    let email = format!("{}@group-dm-live.invalid", uuid::Uuid::new_v4());
    minted_member_with_email(state, PREFIX, &email).await
}

fn listed<'a>(list: &'a Value, id: &str) -> Vec<&'a Value> {
    list["channels"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["id"] == id)
        .collect()
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_group_dm_is_found_or_created_on_its_participants_and_listed_once() {
    let state = live_app_state().await;
    let (me, cookie) = member(&state).await;
    let (maya, maya_cookie) = member(&state).await;
    let (jordan, _) = member(&state).await;
    let agent = format!("live-agent-{}", uuid::Uuid::new_v4());

    // Two people and an agent, unnamed.
    let (status, out) = call_json(
        &state,
        "POST",
        "/api/dms",
        Some(&cookie),
        Some(json!({ "userIds": [maya, jordan], "agents": [agent] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let id = out["channel"]["id"].as_str().unwrap().to_string();
    assert_eq!(out["channel"]["kind"], "dm");
    assert_eq!(out["channel"]["name"], "");

    // The same set in another order (and me listed too) reopens it — and a
    // name given now labels it.
    let (status, again) = call_json(
        &state,
        "POST",
        "/api/dms",
        Some(&cookie),
        Some(
            json!({ "userIds": [jordan, me, maya], "agents": [agent], "name": "  Launch prep  " }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["channel"]["id"], id.as_str());
    assert_eq!(again["channel"]["name"], "Launch prep");

    // Maya sees it ONCE, with everyone else as members and the agent seated —
    // no single peer.
    let (_, list) = call_json(&state, "GET", "/api/channels", Some(&maya_cookie), None).await;
    let rows = listed(&list, &id);
    assert_eq!(rows.len(), 1, "a group DM is one row, not one per member");
    let row = rows[0];
    assert_eq!(row["peer"], Value::Null);
    let mut members: Vec<&str> = row["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["userId"].as_str().unwrap())
        .collect();
    members.sort_unstable();
    let mut want = vec![me.as_str(), jordan.as_str()];
    want.sort_unstable();
    assert_eq!(members, want);
    assert_eq!(row["agents"], json!([agent]));

    // A different set is a different conversation.
    let (_, other) = call_json(
        &state,
        "POST",
        "/api/dms",
        Some(&cookie),
        Some(json!({ "userIds": [maya, jordan] })),
    )
    .await;
    assert_ne!(other["channel"]["id"], id.as_str());
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn one_person_is_the_plain_dm_and_the_refusals_hold() {
    let state = live_app_state().await;
    let (me, cookie) = member(&state).await;
    let (maya, maya_cookie) = member(&state).await;

    // userIds with one person and no agents is the ordinary pair DM — the
    // same row the legacy { userId } body reaches, with its single peer.
    let (_, pair) = call_json(
        &state,
        "POST",
        "/api/dms",
        Some(&cookie),
        Some(json!({ "userIds": [maya] })),
    )
    .await;
    let (_, legacy) = call_json(
        &state,
        "POST",
        "/api/dms",
        Some(&cookie),
        Some(json!({ "userId": maya })),
    )
    .await;
    assert_eq!(pair["channel"]["id"], legacy["channel"]["id"]);
    let id = pair["channel"]["id"].as_str().unwrap();
    let (_, list) = call_json(&state, "GET", "/api/channels", Some(&maya_cookie), None).await;
    let rows = listed(&list, id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["peer"]["userId"], me.as_str());

    let refused = [
        (json!({ "userIds": [] }), StatusCode::BAD_REQUEST),
        (json!({ "agents": ["solo-agent"] }), StatusCode::BAD_REQUEST),
        (
            json!({ "userIds": [uuid::Uuid::new_v4().to_string()] }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "userIds": ["not-a-uuid-at-all-not-a-uuid-at-all1"] }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "userIds": [maya], "name": "x".repeat(81) }),
            StatusCode::BAD_REQUEST,
        ),
    ];
    for (body, want) in refused {
        let (status, out) = call_json(
            &state,
            "POST",
            "/api/dms",
            Some(&cookie),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, want, "{body} → {out}");
    }

    // An agent you may not use is refused, like seating it in a channel.
    sqlx::query(
        "insert into user_agent_access (user_id, agent_model) values ($1::uuid, 'allowed-agent')",
    )
    .bind(&me)
    .execute(&state.pg)
    .await
    .unwrap();
    let (status, _) = call_json(
        &state,
        "POST",
        "/api/dms",
        Some(&cookie),
        Some(json!({ "userIds": [maya], "agents": ["someone-elses-agent"] })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Every other person in a group DM gets ONE row for it (`group_key::dm`),
/// each message coalescing into it; the sender gets none; a member who set
/// the conversation to `mentions` hears only a message that @mentions them.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_group_dm_message_files_one_coalescing_row_per_other_member() {
    let state = live_app_state().await;
    let (me, cookie) = member(&state).await;
    let (maya, maya_cookie) = member(&state).await;
    let (jordan, _) = member(&state).await;
    sqlx::query("update users set name = 'Maya Groupdm' where id = $1::uuid")
        .bind(&maya)
        .execute(&state.pg)
        .await
        .unwrap();
    let (_, out) = call_json(
        &state,
        "POST",
        "/api/dms",
        Some(&cookie),
        Some(json!({ "userIds": [maya, jordan] })),
    )
    .await;
    let id = out["channel"]["id"].as_str().unwrap().to_string();
    let href = format!("/comms/channel/{id}");
    let dm_rows = |user: String| {
        let pg = state.pg.clone();
        let href = href.clone();
        async move {
            sqlx::query_as::<_, (i32, String, Option<String>)>(
                "select count, body, group_key from notifications \
                 where user_id = $1::uuid and kind = 'dm' and href = $2 and read_at is null",
            )
            .bind(&user)
            .bind(&href)
            .fetch_all(&pg)
            .await
            .unwrap()
        }
    };
    let wait_for = |user: String, count: i32| {
        let dm_rows = &dm_rows;
        async move {
            for _ in 0..150 {
                if dm_rows(user.clone())
                    .await
                    .first()
                    .is_some_and(|r| r.0 == count)
                {
                    return true;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            false
        }
    };
    // Maya would rather hear only her @mentions here.
    let (status, _) = call_json(
        &state,
        "PUT",
        &format!("/api/channels/{id}/notify"),
        Some(&maya_cookie),
        Some(json!({ "level": "mentions" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for line in ["first", "second"] {
        let (status, _) = call_json(
            &state,
            "POST",
            &format!("/api/channels/{id}/messages"),
            Some(&cookie),
            Some(json!({ "content": line })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    assert!(
        wait_for(jordan.clone(), 2).await,
        "Jordan's one row counts both"
    );
    let jordan_rows = dm_rows(jordan.clone()).await;
    assert_eq!(jordan_rows.len(), 1);
    assert_eq!(jordan_rows[0].1, "second");
    assert_eq!(jordan_rows[0].2, Some(format!("dm:{id}")));
    assert!(dm_rows(me.clone()).await.is_empty(), "never the sender");
    assert!(
        dm_rows(maya.clone()).await.is_empty(),
        "Maya is at mentions"
    );
    let (status, _) = call_json(
        &state,
        "POST",
        &format!("/api/channels/{id}/messages"),
        Some(&cookie),
        Some(json!({ "content": "@maya a question for you" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(wait_for(maya.clone(), 1).await, "an @mention reaches Maya");
    assert!(wait_for(jordan.clone(), 3).await);
}
