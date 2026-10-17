// /api/memory/{id}. One managed agent's memory, read/written through its
// running container. This agent's managers, and admins — an agent's memory
// is part of the agent (docs/PERMISSIONS.md, "Agent managers").
//
// `?file=memory` (the default) is MEMORY.md, what the agent has learned and
// kept. `?file=profile` is USER.md, the agent's profile of the person it works
// for — the same door, because the only difference is which file in the
// volume is `cat`ed. An unknown value is a 400 rather than a quiet fall back
// to MEMORY.md: a typo must not write the wrong file.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_memory::{MemoryFile, read_memory, write_memory};
use talaria_body::{parse, string_member};
use talaria_error::{house_error, object_or_400};
use talaria_session::require_agent_manager;
use talaria_state::AppState;

/// The `file` query member, defaulting to the memory itself. An absent or
/// empty value is the default; anything unrecognized is the caller's bug.
/// Answers the MESSAGE, the way `talaria_body` does — the 400 envelope is the
/// route's to put on it.
fn which_file(uri: &Uri) -> Result<MemoryFile, &'static str> {
    let raw = uri.query().and_then(|qs| {
        url::form_urlencoded::parse(qs.as_bytes())
            .find(|(k, _)| k == "file")
            .map(|(_, v)| v.into_owned())
    });
    match raw.as_deref().filter(|v| !v.is_empty()) {
        None => Ok(MemoryFile::default()),
        Some(v) => MemoryFile::parse(v).ok_or("file must be \"memory\" or \"profile\""),
    }
}

// doc: Read one of the agent's two memory files. `?file=memory` (the default)
// doc: is MEMORY.md, what the agent has learned and kept; `?file=profile` is
// doc: USER.md, its profile of the person it works for. Any other value is a
// doc: 400. A file the agent has never written reads as empty, not as a miss.

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    uri: Uri,
) -> Result<Response, Response> {
    require_agent_manager(&state, &headers, &id).await?;
    let file = match which_file(&uri) {
        Ok(f) => f,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, msg)),
    };
    Ok(match read_memory(&state.pg, &id, file).await {
        Ok((content, container)) => {
            Json(json!({ "content": content, "container": container })).into_response()
        }
        Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
    })
}

// doc: Replace one memory file whole, snapshotting the result to its own
// doc: history (`?file=` as on the GET). Both files are §-delimited entry
// doc: lists the agent's own memory tool round-trips, and the content is
// doc: written byte for byte — a writer that reflows them costs the agent its
// doc: ability to remember anything.

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    uri: Uri,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_agent_manager(&state, &headers, &id).await?;
    let file = match which_file(&uri) {
        Ok(f) => f,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, msg)),
    };
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // content — required, max 2M; the empty string is legal (min 0: clearing
    // a memory is a write).
    let content = match string_member(obj, "content", 0, 2_000_000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let author = user
        .email
        .as_deref()
        .or(user.name.as_deref())
        .unwrap_or("admin");
    Ok(
        match write_memory(&state.pg, &id, &content, Some(author), file).await {
            Ok(()) => Json(json!({ "ok": true })).into_response(),
            Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_of(uri: &str) -> Result<MemoryFile, &'static str> {
        which_file(&uri.parse::<Uri>().unwrap())
    }

    #[test]
    fn the_default_is_the_memory_and_a_typo_is_a_400() {
        assert_eq!(file_of("/api/memory/x"), Ok(MemoryFile::Memory));
        assert_eq!(file_of("/api/memory/x?file="), Ok(MemoryFile::Memory));
        assert_eq!(file_of("/api/memory/x?file=memory"), Ok(MemoryFile::Memory));
        assert_eq!(
            file_of("/api/memory/x?file=profile"),
            Ok(MemoryFile::Profile)
        );
        assert_eq!(
            file_of("/api/memory/x?file=USER.md"),
            Err("file must be \"memory\" or \"profile\"")
        );
        // another param's value is not the file's
        assert_eq!(
            file_of("/api/memory/x?other=profile"),
            Ok(MemoryFile::Memory)
        );
    }
}
