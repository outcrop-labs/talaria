// /api/working. The tab bar's one read: which views an agent is working in
// right now, as { comms, work, boards }, each a Lead or null. A view the
// server has no predicate for has no key, so the client cannot light it.
//
// Every arm is an engine's own predicate, never a second spelling: the
// sidebar's `working` flag for threads, the room page's dead-stream silence
// for rooms, the research status projection for runs, and the board's
// work-session join for tickets. All read concurrently; none is a write.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use sqlx::PgPool;
use talaria_boards::working_tickets;
use talaria_channels::streaming_channels;
use talaria_conversations::working_conversations;
use talaria_error::internal;
use talaria_research::working_research;
use talaria_session::require_user;
use talaria_state::AppState;

/// One view's working set: how many things an agent is on there, and the
/// freshest of them to name and link.
#[derive(Debug, Serialize, PartialEq)]
pub struct Lead {
    pub count: usize,
    pub label: String,
    pub href: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Working {
    pub comms: Option<Lead>,
    pub work: Option<Lead>,
    pub boards: Option<Lead>,
}

struct Item {
    at: i64,
    label: String,
    href: String,
}

fn lead(items: Vec<Item>) -> Option<Lead> {
    let count = items.len();
    let top = items.into_iter().max_by_key(|i| i.at)?;
    Some(Lead {
        count,
        label: top.label,
        href: top.href,
    })
}

/// The read itself, apart from the session door. `Err` names the arm that
/// failed: one failed arm fails the whole read, because a view reading
/// "nothing working" over a failed query is a silent zero.
pub async fn read(pg: &PgPool, user_id: &str) -> Result<Working, (&'static str, sqlx::Error)> {
    let (chats, rooms, threads, runs, tickets) = tokio::join!(
        working_conversations(pg, user_id, &["chat"]),
        streaming_channels(pg, user_id),
        working_conversations(pg, user_id, &["work", "plan", "research"]),
        working_research(pg, user_id),
        working_tickets(pg, user_id),
    );
    let chats = chats.map_err(|e| ("comms", e))?;
    let rooms = rooms.map_err(|e| ("comms", e))?;
    let threads = threads.map_err(|e| ("work", e))?;
    let runs = runs.map_err(|e| ("work", e))?;
    let tickets = tickets.map_err(|e| ("boards", e))?;

    let comms = chats
        .into_iter()
        .map(|c| Item {
            at: c.streamed_ms,
            href: format!("/comms/agent/{}/{}", c.agent_model, c.id),
            label: c.title.unwrap_or(c.agent_model),
        })
        .chain(rooms.into_iter().map(|r| Item {
            at: r.streamed_ms,
            href: format!("/comms/channel/{}", r.id),
            label: r.name,
        }))
        .collect();

    // A run's discussion thread streaming is the run's own work, so the
    // thread is dropped in favour of the run rather than counted twice.
    let run_threads: Vec<String> = runs
        .iter()
        .filter_map(|r| r.conversation_id.clone())
        .collect();
    let work = threads
        .into_iter()
        .filter(|t| !run_threads.contains(&t.id))
        .map(|t| Item {
            at: t.streamed_ms,
            href: format!("/work/{}", t.id),
            label: t.title.unwrap_or(t.agent_model),
        })
        .chain(runs.into_iter().map(|r| Item {
            at: r.updated_ms,
            href: format!("/research/{}", r.id),
            label: r.title,
        }))
        .collect();

    let boards = tickets
        .into_iter()
        .map(|t| Item {
            at: t.updated_ms,
            href: format!("/boards/{}/{}", t.board_id, t.id),
            label: t.title,
        })
        .collect();

    Ok(Working {
        comms: lead(comms),
        work: lead(work),
        boards: lead(boards),
    })
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    match read(&state.pg, &user.id).await {
        Ok(working) => Ok(Json(working).into_response()),
        Err((arm, e)) => Ok(internal(&format!("[working] {arm} arm failed"), e)),
    }
}

#[cfg(test)]
#[path = "working_live.rs"]
mod working_live;
