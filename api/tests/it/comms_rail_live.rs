// Live-DB + live-Redis proof for the Comms rail's row menus: the caller's
// pins (`/api/me/pins`), their agent-DM mutes (`/api/me/agent-mutes`) and the
// quiet row a muted agent's reply files, mark-unread on a thread and on a
// channel (`…/unread`), and the caller's notify level on the channel list.
//
// WHY ONLY A REAL DATABASE PROVES IT. Each claim is a WHERE or an upsert: that
// a pin is the caller's own row and keeps its place, that the cap and the
// insert read the same rows, that the step back lands exactly one countable
// message behind the newest under the same predicate the unread badge counts
// with, and that the producer reads the mute at the moment it files.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it comms_rail_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use talaria_api::conversations::{create_conversation, mark_conversation_read};
use talaria_api::notify::{NotifyDeps, notify_agent_reply};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "comms-rail-live";

/// A throwaway person with a session, unique per call: the binary runs its
/// cases concurrently and each reads a whole per-person list.
async fn member(state: &AppState, tag: &str) -> (String, String) {
    let email = format!("{}@{PREFIX}-{tag}.invalid", uuid::Uuid::new_v4());
    minted_member_with_email(state, &format!("{PREFIX}:{tag}"), &email).await
}

async fn sweep(state: &AppState, users: &[&str]) {
    for u in users {
        sqlx::query("delete from users where id = $1::uuid")
            .bind(u)
            .execute(&state.pg)
            .await
            .unwrap();
    }
}

fn pins_of(v: &Value) -> Vec<String> {
    v["pins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
#[ignore = "needs a live dev database and Redis (source ui/.env)"]
async fn pins_are_the_callers_own_in_the_order_they_were_made() {
    let state = live_app_state().await;
    let (me, cookie) = member(&state, "pins").await;
    let (other, other_cookie) = member(&state, "pins-other").await;
    let ch = uuid::Uuid::new_v4().to_string();
    let th = uuid::Uuid::new_v4().to_string();

    let (s, v) = call_json(&state, "GET", "/api/me/pins", Some(&cookie), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(pins_of(&v), Vec::<String>::new());

    for key in [
        format!("channel:{ch}"),
        "agent:developer".into(),
        format!("thread:{th}"),
    ] {
        let (s, v) = call_json(
            &state,
            "POST",
            "/api/me/pins",
            Some(&cookie),
            Some(json!({ "key": key })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }
    // Pinning again is a no-op that keeps the original place, and an
    // upper-case uuid is the same pin.
    let (s, _) = call_json(
        &state,
        "POST",
        "/api/me/pins",
        Some(&cookie),
        Some(json!({ "key": format!("channel:{}", ch.to_uppercase()) })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = call_json(&state, "GET", "/api/me/pins", Some(&cookie), None).await;
    assert_eq!(
        pins_of(&v),
        vec![
            format!("channel:{ch}"),
            "agent:developer".into(),
            format!("thread:{th}")
        ]
    );

    // Another person's rail is untouched by any of it.
    let (_, v) = call_json(&state, "GET", "/api/me/pins", Some(&other_cookie), None).await;
    assert!(pins_of(&v).is_empty());

    // Grammar is a 400 with the sentence; no session is a 401.
    let (s, v) = call_json(
        &state,
        "POST",
        "/api/me/pins",
        Some(&cookie),
        Some(json!({ "key": "channel:x" })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "a channel pin names a uuid");
    let (s, _) = call_json(
        &state,
        "POST",
        "/api/me/pins",
        Some(&cookie),
        Some(json!({ "key": "person:x" })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call_json(&state, "GET", "/api/me/pins", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Unpin, and unpinning what is not pinned answers the same.
    for _ in 0..2 {
        let (s, _) = call_json(
            &state,
            "DELETE",
            "/api/me/pins",
            Some(&cookie),
            Some(json!({ "key": "agent:developer" })),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }
    let (_, v) = call_json(&state, "GET", "/api/me/pins", Some(&cookie), None).await;
    assert_eq!(
        pins_of(&v),
        vec![format!("channel:{ch}"), format!("thread:{th}")]
    );

    // The cap: fill to 200, and the 201st is refused while a held key still
    // answers ok.
    sqlx::query(
        "insert into comms_pins (user_id, pin_key) \
         select $1::uuid, 'agent:filler-' || g from generate_series(1, 198) g",
    )
    .bind(&me)
    .execute(&state.pg)
    .await
    .unwrap();
    let (s, v) = call_json(
        &state,
        "POST",
        "/api/me/pins",
        Some(&cookie),
        Some(json!({ "key": "agent:one-more" })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let (s, _) = call_json(
        &state,
        "POST",
        "/api/me/pins",
        Some(&cookie),
        Some(json!({ "key": format!("thread:{th}") })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    sweep(&state, &[&me, &other]).await;
}

#[tokio::test]
#[ignore = "needs a live dev database and Redis (source ui/.env)"]
async fn a_muted_agents_reply_files_quietly_until_the_mute_is_lifted() {
    let state = live_app_state().await;
    let (me, cookie) = member(&state, "mute").await;
    let model = format!("{PREFIX}-agent-{}", uuid::Uuid::new_v4());

    let (s, v) = call_json(&state, "GET", "/api/me/agent-mutes", Some(&cookie), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["mutes"], json!([]));

    let until = "2099-01-01T00:00:00.000Z";
    let (s, v) = call_json(
        &state,
        "PUT",
        "/api/me/agent-mutes",
        Some(&cookie),
        Some(json!({ "agentModel": model, "mutedUntil": until })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["mutedUntil"], until);
    let (_, v) = call_json(&state, "GET", "/api/me/agent-mutes", Some(&cookie), None).await;
    assert_eq!(
        v["mutes"],
        json!([{ "agentModel": model, "mutedUntil": until }])
    );

    // A past time is a 400, and a mute that ran out lists as none.
    let (s, _) = call_json(
        &state,
        "PUT",
        "/api/me/agent-mutes",
        Some(&cookie),
        Some(json!({ "agentModel": model, "mutedUntil": "2001-01-01T00:00:00.000Z" })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // The producer reads the mute: the reply still files, as `feed` (quiet).
    let deps = NotifyDeps::publishing(state.pg.clone(), None);
    let chat = create_conversation(&state.pg, &me, &model, "Muted chat", "chat", None)
        .await
        .unwrap();
    let reply = |seq: i32| {
        let pg = state.pg.clone();
        let chat = chat.clone();
        async move {
            let (id,): (String,) = sqlx::query_as(
                "insert into messages (conversation_id, seq, role, status, content) \
                 values ($1::uuid, $2, 'assistant', 'complete', 'an answer') returning id::text",
            )
            .bind(&chat)
            .bind(seq)
            .fetch_one(&pg)
            .await
            .unwrap();
            id
        }
    };
    let first = reply(1).await;
    notify_agent_reply(&deps, &chat, &first).await;
    let alerts = |pg: sqlx::PgPool, user: String| async move {
        sqlx::query_scalar::<_, String>(
            "select alert from notifications where user_id = $1::uuid and kind = 'agent-reply' \
             order by created_at",
        )
        .bind(user)
        .fetch_all(&pg)
        .await
        .unwrap()
    };
    assert_eq!(
        alerts(state.pg.clone(), me.clone()).await,
        vec!["feed".to_string()]
    );

    // DMs only: the same muted agent's reply in a PLAN is not its DM, and
    // rings as ever.
    let plan = create_conversation(&state.pg, &me, &model, "Muted agent's plan", "plan", None)
        .await
        .unwrap();
    let (plan_reply,): (String,) = sqlx::query_as(
        "insert into messages (conversation_id, seq, role, status, content) \
         values ($1::uuid, 1, 'assistant', 'complete', 'a plan answer') returning id::text",
    )
    .bind(&plan)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    notify_agent_reply(&deps, &plan, &plan_reply).await;
    assert_eq!(
        alerts(state.pg.clone(), me.clone()).await,
        vec!["feed".to_string(), "banner".to_string()],
        "a plan reply from a muted agent still rings"
    );

    // Unmuted, and read past the first, the next reply rings again.
    let (s, v) = call_json(
        &state,
        "PUT",
        "/api/me/agent-mutes",
        Some(&cookie),
        Some(json!({ "agentModel": model, "mutedUntil": null })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["mutedUntil"], Value::Null);
    let (_, v) = call_json(&state, "GET", "/api/me/agent-mutes", Some(&cookie), None).await;
    assert_eq!(v["mutes"], json!([]));
    sqlx::query("update notifications set read_at = now() where user_id = $1::uuid")
        .bind(&me)
        .execute(&state.pg)
        .await
        .unwrap();
    mark_conversation_read(&state.pg, &chat, &me, 1)
        .await
        .unwrap();
    let second = reply(2).await;
    notify_agent_reply(&deps, &chat, &second).await;
    assert_eq!(
        alerts(state.pg.clone(), me.clone()).await,
        vec![
            "feed".to_string(),
            "banner".to_string(),
            "banner".to_string()
        ]
    );

    // A write prunes the person's expired mutes.
    sqlx::query(
        "insert into agent_dm_mutes (user_id, agent_model, muted_until) \
         values ($1::uuid, 'long-gone', now() - interval '1 day')",
    )
    .bind(&me)
    .execute(&state.pg)
    .await
    .unwrap();
    let (s, _) = call_json(
        &state,
        "PUT",
        "/api/me/agent-mutes",
        Some(&cookie),
        Some(json!({ "agentModel": model, "mutedUntil": "2099-01-01T00:00:00.000Z" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let rows: i64 =
        sqlx::query_scalar("select count(*) from agent_dm_mutes where user_id = $1::uuid")
            .bind(&me)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    assert_eq!(rows, 1, "the expired row went with the write");

    sweep(&state, &[&me]).await;
}

/// The caller's unread pill for one thread, from the list the rail reads.
async fn thread_unread(state: &AppState, cookie: &str, id: &str) -> i64 {
    let (_, v) = call_json(
        state,
        "GET",
        "/api/conversations?kind=chat",
        Some(cookie),
        None,
    )
    .await;
    v["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .and_then(|c| c["unreadCount"].as_i64())
        .unwrap_or(0)
}

#[tokio::test]
#[ignore = "needs a live dev database and Redis (source ui/.env)"]
async fn mark_unread_steps_a_thread_back_one_countable_turn() {
    let state = live_app_state().await;
    let (me, cookie) = member(&state, "unread-thread").await;
    let (stranger, stranger_cookie) = member(&state, "unread-stranger").await;
    let chat = create_conversation(
        &state.pg,
        &me,
        "assistant-live",
        "Unread chat",
        "chat",
        None,
    )
    .await
    .unwrap();

    // An empty thread has nothing to mark: 0, and nothing written.
    let path = format!("/api/conversations/{chat}/unread");
    let (s, v) = call_json(&state, "POST", &path, Some(&cookie), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["marked"], false);

    for (seq, role, author) in [
        (0, "user", Some(&me)),
        (1, "assistant", None),
        (2, "user", Some(&me)),
    ] {
        sqlx::query(
            "insert into messages (conversation_id, seq, role, status, content, author_user_id) \
             values ($1::uuid, $2, $3, 'complete', 'x', $4::uuid)",
        )
        .bind(&chat)
        .bind(seq)
        .bind(role)
        .bind(author)
        .execute(&state.pg)
        .await
        .unwrap();
    }
    mark_conversation_read(&state.pg, &chat, &me, 2)
        .await
        .unwrap();
    assert_eq!(thread_unread(&state, &cookie, &chat).await, 0);

    // The newest turn that counts is the agent's at seq 1 — my own seq 2
    // never counts for me — so the cursor lands at 0 and the pill reads 1.
    let (s, v) = call_json(&state, "POST", &path, Some(&cookie), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["marked"], true);
    assert_eq!(thread_unread(&state, &cookie, &chat).await, 1);
    // And reading again clears it, as ever (the cursor only advances there).
    mark_conversation_read(&state.pg, &chat, &me, 2)
        .await
        .unwrap();
    assert_eq!(thread_unread(&state, &cookie, &chat).await, 0);

    // NEVER FORWARD. Two more agent turns land (3 and 5, my 4 between) while
    // the rail still shows 0, and Mark unread is clicked against that stale
    // count: the newest countable turn is 5, but the cursor at 2 is already
    // behind 4, so it stays at 2 and both replies stay unread.
    for (seq, role, author) in [
        (3, "assistant", None),
        (4, "user", Some(&me)),
        (5, "assistant", None),
    ] {
        sqlx::query(
            "insert into messages (conversation_id, seq, role, status, content, author_user_id) \
             values ($1::uuid, $2, $3, 'complete', 'x', $4::uuid)",
        )
        .bind(&chat)
        .bind(seq)
        .bind(role)
        .bind(author)
        .execute(&state.pg)
        .await
        .unwrap();
    }
    assert_eq!(thread_unread(&state, &cookie, &chat).await, 2);
    let (s, v) = call_json(&state, "POST", &path, Some(&cookie), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        thread_unread(&state, &cookie, &chat).await,
        2,
        "nothing was marked read"
    );
    let cursor: i32 = sqlx::query_scalar(
        "select last_read_seq from conversation_reads where conversation_id = $1::uuid and user_id = $2::uuid",
    )
    .bind(&chat)
    .bind(&me)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    assert_eq!(cursor, 2);

    // Someone with no access: 403, as the read answers; a mangled id is 404.
    let (s, _) = call_json(&state, "POST", &path, Some(&stranger_cookie), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call_json(
        &state,
        "POST",
        "/api/conversations/nope/unread",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    sweep(&state, &[&me, &stranger]).await;
}

/// One channel row from the caller's listing.
async fn listed(state: &AppState, cookie: &str, id: &str) -> Value {
    let (_, v) = call_json(state, "GET", "/api/channels", Some(cookie), None).await;
    v["channels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .cloned()
        .expect("the channel lists for its member")
}

#[tokio::test]
#[ignore = "needs a live dev database and Redis (source ui/.env)"]
async fn mark_unread_steps_a_channel_back_and_the_list_carries_the_notify_level() {
    let state = live_app_state().await;
    let (me, cookie) = member(&state, "unread-channel").await;
    let (mate, _) = member(&state, "unread-mate").await;
    let (outsider, outsider_cookie) = member(&state, "unread-outsider").await;
    let my_email: String = sqlx::query_scalar("select email from users where id = $1::uuid")
        .bind(&me)
        .fetch_one(&state.pg)
        .await
        .unwrap();
    let mate_email: String = sqlx::query_scalar("select email from users where id = $1::uuid")
        .bind(&mate)
        .fetch_one(&state.pg)
        .await
        .unwrap();
    let ch = talaria_api::channels::create_channel(
        &state.pg,
        &me,
        &format!("rail-{}", &uuid::Uuid::new_v4().to_string()[..8]),
        None,
        "channel",
    )
    .await
    .unwrap()
    .id;

    // The level rides the listing: a channel's default is mentions.
    let row = listed(&state, &cookie, &ch).await;
    assert_eq!(
        row["notify"],
        json!({ "level": "mentions", "mutedUntil": null, "settable": true })
    );
    let (s, _) = call_json(
        &state,
        "PUT",
        &format!("/api/channels/{ch}/notify"),
        Some(&cookie),
        Some(json!({ "level": "all", "mutedUntil": "2099-01-01T00:00:00.000Z" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let row = listed(&state, &cookie, &ch).await;
    assert_eq!(
        row["notify"],
        json!({ "level": "all", "mutedUntil": "2099-01-01T00:00:00.000Z", "settable": true })
    );

    // Feed: the mate at 1, me at 2, a reply under 1 at 3. All read.
    for (seq, author, root) in [
        (1, &mate_email, false),
        (2, &my_email, false),
        (3, &mate_email, true),
    ] {
        sqlx::query(
            "insert into channel_messages (channel_id, seq, author_type, author, content, thread_root_id) \
             values ($1::uuid, $2, 'user', $3, 'x', \
               case when $4 then (select id from channel_messages where channel_id = $1::uuid and seq = 1) end)",
        )
        .bind(&ch)
        .bind(seq)
        .bind(author)
        .bind(root)
        .execute(&state.pg)
        .await
        .unwrap();
    }
    sqlx::query("update channel_members set last_read_seq = 3 where channel_id = $1::uuid and user_id = $2::uuid")
        .bind(&ch)
        .bind(&me)
        .execute(&state.pg)
        .await
        .unwrap();
    assert_eq!(listed(&state, &cookie, &ch).await["unreadCount"], 0);

    // The newest message that counts, by the badge's own rule, is the mate's
    // reply at 3 (mine at 2 never counts; no thread cursor of mine covers the
    // reply): the cursor lands at 2 and the badge reads exactly 1.
    let path = format!("/api/channels/{ch}/unread");
    let (s, v) = call_json(&state, "POST", &path, Some(&cookie), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["marked"], true);
    assert_eq!(listed(&state, &cookie, &ch).await["unreadCount"], 1);
    // Read it all again, this time with the thread read in its panel too:
    // the reply no longer counts, so the newest countable is the feed
    // message at 1, the cursor lands at 0, and the badge is 1 again.
    let my_cursor = |seq: i32| {
        let pg = state.pg.clone();
        let (ch, me) = (ch.clone(), me.clone());
        async move {
            sqlx::query("update channel_members set last_read_seq = $3 where channel_id = $1::uuid and user_id = $2::uuid")
                .bind(&ch)
                .bind(&me)
                .bind(seq)
                .execute(&pg)
                .await
                .unwrap();
        }
    };
    my_cursor(3).await;
    sqlx::query(
        "insert into channel_thread_reads (thread_root_id, user_id, last_read_seq) \
         select id, $2::uuid, 3 from channel_messages where channel_id = $1::uuid and seq = 1",
    )
    .bind(&ch)
    .bind(&me)
    .execute(&state.pg)
    .await
    .unwrap();
    assert_eq!(listed(&state, &cookie, &ch).await["unreadCount"], 0);
    let (s, _) = call_json(&state, "POST", &path, Some(&cookie), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(listed(&state, &cookie, &ch).await["unreadCount"], 1);

    // NEVER FORWARD: with the cursor at 0 already, two more of the mate's
    // feed messages (4, 5) make the badge 3; a Mark unread clicked against
    // a stale badge leaves the cursor at 0 and all three unread.
    for seq in [4, 5] {
        sqlx::query(
            "insert into channel_messages (channel_id, seq, author_type, author, content) \
             values ($1::uuid, $2, 'user', $3, 'x')",
        )
        .bind(&ch)
        .bind(seq)
        .bind(&mate_email)
        .execute(&state.pg)
        .await
        .unwrap();
    }
    assert_eq!(listed(&state, &cookie, &ch).await["unreadCount"], 3);
    let (s, _) = call_json(&state, "POST", &path, Some(&cookie), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        listed(&state, &cookie, &ch).await["unreadCount"],
        3,
        "nothing was marked read"
    );

    // Not a member: 403, whether or not it exists (the channels family).
    let (s, _) = call_json(&state, "POST", &path, Some(&outsider_cookie), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call_json(
        &state,
        "POST",
        &format!("/api/channels/{}/unread", uuid::Uuid::new_v4()),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    sqlx::query("delete from channels where id = $1::uuid")
        .bind(&ch)
        .execute(&state.pg)
        .await
        .unwrap();
    sweep(&state, &[&me, &mate, &outsider]).await;
}
