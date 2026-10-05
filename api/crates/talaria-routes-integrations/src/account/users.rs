// GET /api/users. Everyone who has signed in, for the people pickers and the
// Comms sidebar: id, email, name, the effective picture (an uploaded photo,
// else the sign-in provider's), status emoji + text, and `online` — whether
// they pinged `PUT /api/me/presence` within the last 90s. Any signed-in user —
// and agents (their own tak_ key or the fleet key): they need the directory to
// resolve "email Priya" or "add Priya to the board" into an address.
//
// Presence is decoration, never a dependency: one MGET over every person's
// key, and when Redis is unreachable or the read fails the directory still
// answers, with everyone `online: false`.
//
// This route once doubled as mcp/'s fleet-wide auth oracle (the resident GET
// /api/users with the agent's credential to check the fleet was still talking
// to its org). The oracle has its own home now — GET /api/agent/whoami, see
// routes/agents/agent_whoami.rs — and narrowing THIS route to a people-picker
// decision no longer takes the toolkit dark.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use talaria_agent_auth::agent_caller;
use talaria_error::internal;
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_users::{list_directory, online_flags, presence_key};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectoryUser {
    id: String,
    email: Option<String>,
    name: Option<String>,
    picture: Option<String>,
    status_emoji: Option<String>,
    status_text: Option<String>,
    online: bool,
}

#[derive(serde::Serialize)]
struct UsersBody {
    users: Vec<DirectoryUser>,
}

/// One MGET for everyone's presence key. None on any failure — the caller
/// maps that to all-offline.
async fn read_presence(state: &AppState, ids: &[String]) -> Option<Vec<Option<String>>> {
    if ids.is_empty() {
        return Some(Vec::new());
    }
    let mut conn = match state.redis().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("[users] presence redis unavailable: {e}");
            return None;
        }
    };
    let keys: Vec<String> = ids.iter().map(|id| presence_key(id)).collect();
    match redis::cmd("MGET")
        .arg(&keys)
        .query_async::<Vec<Option<String>>>(&mut conn)
        .await
    {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("[users] presence read failed: {e}");
            None
        }
    }
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    // A presented agent credential that is REJECTED returns its refusal —
    // falling through would turn a forgery into a quiet 401.
    match agent_caller(&state.pg, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            require_user(&state, &headers).await?;
        }
        Err(resp) => return Err(resp),
    }
    let rows = match list_directory(&state.pg).await {
        Ok(rows) => rows,
        Err(e) => return Ok(internal("[users] directory query failed", e)),
    };
    let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let online = online_flags(ids.len(), read_presence(&state, &ids).await);
    Ok(Json(UsersBody {
        users: rows
            .into_iter()
            .zip(online)
            .map(|(r, online)| DirectoryUser {
                id: r.id,
                email: r.email,
                name: r.name,
                picture: r.face.picture,
                status_emoji: r.face.status_emoji,
                status_text: r.face.status_text,
                online,
            })
            .collect(),
    })
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// State whose Redis is a closed port and whose Postgres is never
    /// touched (connect_lazy): exactly the outage the directory must survive.
    fn dead_redis_state() -> AppState {
        let cfg = std::sync::Arc::new(
            talaria_config::Config::from_parts(
                "postgres://nobody:nobody@127.0.0.1:1/none".into(),
                "redis://127.0.0.1:1/1".into(),
                "test-root".into(),
                String::new(),
                String::new(),
                "5274".into(),
            )
            .expect("test config"),
        );
        let pg = sqlx::PgPool::connect_lazy(&cfg.database_url).expect("lazy pool");
        AppState::new(pg, cfg)
    }

    #[tokio::test]
    async fn presence_outage_reads_as_everyone_offline() {
        let state = dead_redis_state();
        let ids = vec!["u1".to_string(), "u2".to_string()];
        let read = read_presence(&state, &ids).await;
        assert!(
            read.is_none(),
            "a closed redis is a failed read, not a panic"
        );
        assert_eq!(online_flags(ids.len(), read), vec![false, false]);
        // Nobody to ask about: no round trip at all.
        assert_eq!(read_presence(&state, &[]).await, Some(Vec::new()));
    }

    #[test]
    fn directory_row_wire_is_camel_case_in_order() {
        let u = DirectoryUser {
            id: "u1".into(),
            email: Some("a@x".into()),
            name: None,
            picture: Some("/api/users/u1/avatar?v=abcd1234".into()),
            status_emoji: Some("📅".into()),
            status_text: Some("In a meeting".into()),
            online: true,
        };
        assert_eq!(
            serde_json::to_string(&u).unwrap(),
            r#"{"id":"u1","email":"a@x","name":null,"picture":"/api/users/u1/avatar?v=abcd1234","statusEmoji":"📅","statusText":"In a meeting","online":true}"#
        );
    }
}
