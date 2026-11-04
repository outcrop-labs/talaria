// Live-Redis proof of the stop a page sends by its send's own name (AP3,
// AP4): POST /api/chat/turns/{id}/stop, and the optional `{ turnId }` on
// POST /api/conversations/{id}/stop. talaria-chat-persist's `turn_stop`
// unit tests cover the table; these pin the routes' wiring over it:
//
// - a malformed name is the 404 every malformed path id is, and no session
//   is the 401;
// - the thread stop refuses a body that is not an object, or a `turnId`
//   that is not a uuid, with a 400 before it reads the thread;
// - a name another person's send carries is that person's: the stop is the
//   409 a turn that ended gets, at once, and the send is not handed a stop;
// - a stop whose request goes away before its send comes is held, a second
//   stop under the name answers `held` at once, and the send, when it
//   comes, is handed the stop.
//
// The table is this process's (the router runs in-process here, as it does
// in the api), so the test stands in for the send with `turn_sent`, exactly
// the call /api/chat makes once it has checked access and claimed the
// thread. Nothing here reaches an agent.
// House rule: #[ignore]d; CI's api-integration job runs it.
//
//   source ui/.env && cargo test -p talaria-api --test it turn_stop_live:: -- --ignored

use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;
use talaria_api::session::{SessionUser, create_session};
use talaria_api::state::AppState;
use talaria_chat_persist::turn_sent;
use tokio::time::Instant;

use crate::support::{call_json, live_app_state};

const PREFIX: &str = "turn-stop-live";

async fn cleanup(state: &AppState) {
    sqlx::query("delete from users where sub like $1")
        .bind(format!("{PREFIX}:%"))
        .execute(&state.pg)
        .await
        .unwrap();
}

/// A member with a session minted the way the auth routes mint one, and
/// their id.
async fn minted_member(state: &AppState) -> (String, String) {
    let sub = format!("{PREFIX}:{}", uuid::Uuid::new_v4());
    let (id,): (String,) =
        sqlx::query_as("insert into users (sub, role) values ($1, 'member') returning id::text")
            .bind(&sub)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    let user = SessionUser {
        id: id.clone(),
        sub,
        email: None,
        name: None,
        picture: None,
        role: "member".into(),
        provider: "password".into(),
    };
    let sid = create_session(state, &user)
        .await
        .expect("session mints — needs REDIS_URL");
    (id, format!("talaria_session={sid}"))
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_stop_by_its_sends_name_is_gated_held_and_only_the_senders() {
    let state = live_app_state().await;
    cleanup(&state).await;
    let (me, my_cookie) = minted_member(&state).await;
    let (them, _their_cookie) = minted_member(&state).await;
    let turn_stop = |name: &str| format!("/api/chat/turns/{name}/stop");

    // The gates.
    let (status, _) = call_json(
        &state,
        "POST",
        &turn_stop("not-a-uuid"),
        Some(&my_cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a malformed name");
    let unnamed = uuid::Uuid::new_v4().to_string();
    let (status, _) = call_json(&state, "POST", &turn_stop(&unnamed), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no session");
    let thread_stop = format!("/api/conversations/{}/stop", uuid::Uuid::new_v4());
    for body in [
        json!({ "turnId": 5 }),
        json!({ "turnId": "not-a-uuid" }),
        json!({ "turnId": ["x"] }),
        json!([1]),
        json!("stop"),
    ] {
        let (status, answer) = call_json(
            &state,
            "POST",
            &thread_stop,
            Some(&my_cookie),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} → {answer}");
    }
    // No name, or a null one, is the old stop: past the body, to the
    // thread's access gate, which a thread nobody has refuses.
    for body in [None, Some(json!({})), Some(json!({ "turnId": null }))] {
        let (status, answer) =
            call_json(&state, "POST", &thread_stop, Some(&my_cookie), body.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body:?} → {answer}");
    }

    // Another person's send under a name: my stop under it is nothing, at
    // once, and their send is handed no stop of mine.
    let theirs = uuid::Uuid::new_v4().to_string();
    let thread = uuid::Uuid::new_v4().to_string();
    let their_send = turn_sent(&theirs, &them, &thread);
    let started = Instant::now();
    let (status, answer) =
        call_json(&state, "POST", &turn_stop(&theirs), Some(&my_cookie), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    assert_eq!(answer["error"], "nothing is streaming", "{answer}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "it did not wait for a send"
    );
    assert!(!their_send.stop_held());
    drop(their_send);

    // My stop whose request goes away before the send comes: held.
    let mine = uuid::Uuid::new_v4().to_string();
    let gone = tokio::time::timeout(
        Duration::from_secs(2),
        call_json(&state, "POST", &turn_stop(&mine), Some(&my_cookie), None),
    )
    .await;
    assert!(gone.is_err(), "the stop waits for its send: {gone:?}");
    // A second stop under the name finds it held, without the wait.
    let started = Instant::now();
    let (status, answer) =
        call_json(&state, "POST", &turn_stop(&mine), Some(&my_cookie), None).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer, json!({ "ok": true, "hermes": false, "held": true }));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a held stop does not wait again"
    );
    // Someone else's send under my held name is not handed my stop, and
    // mine, when it comes, is.
    let squatter = turn_sent(&mine, &them, &thread);
    assert!(!squatter.stop_held());
    drop(squatter);
    let other = uuid::Uuid::new_v4().to_string();
    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        call_json(&state, "POST", &turn_stop(&other), Some(&my_cookie), None),
    )
    .await;
    let my_send = turn_sent(&other, &me, &thread);
    assert!(my_send.stop_held(), "the send picks up its own held stop");
    drop(my_send);

    cleanup(&state).await;
}
