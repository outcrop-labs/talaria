// Live-DB proof of the artifact list's two narrowings: `since` keeps only
// what was updated at or after the instant, `fields=heads` drops the body and
// nothing else, and neither lets a viewer see more than the whole list shows
// them — someone else's private file stays out, a granted one and an org one
// come in. The handler's own answer once its caller is known (`list_for`,
// with the person viewer the session builds). `#[ignore]`d — it needs a
// database — and CI's live-tests job runs it
// (`cargo test --workspace --lib -- --ignored`):
//
//   DATABASE_URL=postgres://… cargo test -p talaria-routes-knowledge --lib -- --ignored artifacts_live

use std::collections::HashMap;

use serde_json::Value;
use sqlx::PgPool;
use talaria_session::SessionUser;

use super::{ListQuery, list_for, list_query, person_viewer};

/// A tag no other run's rows share, minted by the database itself.
async fn fresh_tag(pg: &PgPool) -> String {
    sqlx::query_scalar("select left(replace(gen_random_uuid()::text, '-', ''), 12)")
        .fetch_one(pg)
        .await
        .unwrap()
}

async fn cleanup(pg: &PgPool, tag: &str) {
    let like = format!("artifacts-live-{tag}-%");
    sqlx::query(
        "delete from kb_editors where item_type = 'artifact' and item_id in \
         (select id from artifacts where title like $1)",
    )
    .bind(&like)
    .execute(pg)
    .await
    .unwrap();
    sqlx::query("delete from artifacts where title like $1")
        .bind(&like)
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from users where sub like $1")
        .bind(format!("artifacts-live:{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
}

/// A member, as their session carries them.
async fn person(pg: &PgPool, tag: &str, who: &str) -> SessionUser {
    let sub = format!("artifacts-live:{tag}:{who}");
    let id: String = sqlx::query_scalar(
        "insert into users (sub, email, name, role) values ($1, $1, $2, 'member') returning id::text",
    )
    .bind(&sub)
    .bind(who)
    .fetch_one(pg)
    .await
    .expect("the user");
    SessionUser {
        id,
        sub: sub.clone(),
        email: Some(sub),
        name: Some(who.to_string()),
        picture: None,
        role: "member".into(),
        provider: "password".into(),
    }
}

/// An artifact `owner` made, last updated `hours_ago` hours back.
async fn artifact(
    pg: &PgPool,
    tag: &str,
    name: &str,
    owner: &str,
    visibility: &str,
    hours_ago: f64,
) -> String {
    sqlx::query_scalar(
        "insert into artifacts (title, body, visibility, owner_user_id, created_by, created_at, updated_at) \
         values ($1, $2, $3, $4::uuid, $4, now() - make_interval(secs => $5 * 3600), \
                 now() - make_interval(secs => $5 * 3600)) \
         returning id::text",
    )
    .bind(format!("artifacts-live-{tag}-{name}"))
    .bind(format!("the body of {name}"))
    .bind(visibility)
    .bind(owner)
    .bind(hours_ago)
    .fetch_one(pg)
    .await
    .expect("the artifact")
}

/// This run's rows from the list `reader` gets for `query`, by the name each
/// was made with.
async fn listed(
    pg: &PgPool,
    tag: &str,
    reader: &SessionUser,
    query: &[(&str, &str)],
) -> HashMap<String, Value> {
    let q: HashMap<String, String> = query
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let query: ListQuery = list_query(&q).expect("a valid query");
    let viewer = person_viewer(pg, reader).await.unwrap();
    let artifacts = list_for(pg, &viewer, &query).await.unwrap();
    let prefix = format!("artifacts-live-{tag}-");
    artifacts
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| {
            let name = a["title"].as_str()?.strip_prefix(&prefix)?.to_string();
            Some((name, a.clone()))
        })
        .collect()
}

fn names(rows: &HashMap<String, Value>) -> Vec<&str> {
    let mut n: Vec<&str> = rows.keys().map(String::as_str).collect();
    n.sort_unstable();
    n
}

#[tokio::test]
#[ignore]
async fn artifacts_live_since_and_heads_narrow_without_widening() {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (a migrated database) to run the ignored live tests");
    let pg = PgPool::connect(&url).await.expect("connect");
    let tag = fresh_tag(&pg).await;
    let reader = person(&pg, &tag, "reader").await;
    let other = person(&pg, &tag, "other").await;
    let (reader_id, other_id) = (reader.id.as_str(), other.id.as_str());

    artifact(&pg, &tag, "mine-today", reader_id, "private", 1.0).await;
    artifact(&pg, &tag, "theirs-today", other_id, "private", 1.0).await;
    let granted = artifact(&pg, &tag, "granted-today", other_id, "private", 2.0).await;
    sqlx::query(
        "insert into kb_editors (item_type, item_id, principal_type, principal_id, role) \
         values ('artifact', $1::uuid, 'user', $2, 'viewer')",
    )
    .bind(&granted)
    .bind(reader_id)
    .execute(&pg)
    .await
    .unwrap();
    artifact(&pg, &tag, "org-today", other_id, "org", 3.0).await;
    artifact(&pg, &tag, "org-last-week", other_id, "org", 24.0 * 7.0).await;

    let since: String = sqlx::query_scalar(
        "select to_char((now() - interval '1 day') at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')",
    )
    .fetch_one(&pg)
    .await
    .unwrap();

    let whole = listed(&pg, &tag, &reader, &[]).await;
    assert_eq!(
        names(&whole),
        ["granted-today", "mine-today", "org-last-week", "org-today"],
        "the whole list: someone else's private file stays out"
    );
    let heads = listed(
        &pg,
        &tag,
        &reader,
        &[("since", since.as_str()), ("fields", "heads")],
    )
    .await;
    assert_eq!(
        names(&heads),
        ["granted-today", "mine-today", "org-today"],
        "since keeps what changed in the last day, and the read filter still holds"
    );
    for (name, head) in &heads {
        assert!(head.get("body").is_none(), "{name} carries no body: {head}");
        let mut row = whole[name].clone();
        assert_eq!(
            row.as_object_mut().unwrap().remove("body"),
            Some(Value::String(format!("the body of {name}")))
        );
        assert_eq!(head, &row, "{name}: a head is the whole row less its body");
    }
    let since_whole = listed(&pg, &tag, &reader, &[("since", since.as_str())]).await;
    assert_eq!(names(&since_whole), names(&heads));
    for (name, row) in &since_whole {
        assert_eq!(row, &whole[name], "since alone keeps the whole row");
    }

    // A malformed narrowing is refused before any read: the handler's 400.
    let bad = HashMap::from([("since".to_string(), "yesterday".to_string())]);
    assert!(list_query(&bad).is_err());

    cleanup(&pg, &tag).await;
}
