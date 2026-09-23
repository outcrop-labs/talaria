// POST /api/chat/chips/resolve — titles for platform links pasted in chat.
// A link the caller cannot read comes back without a title: the chip shows
// the kind, not the name.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_body::parse;
use talaria_chips::resolve_titles;
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

// doc: Titles for platform links pasted in chat. A link the caller cannot
// read comes back without a title — the chip shows the kind, not the name.

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let hrefs = match obj.get("hrefs").and_then(Value::as_array) {
        Some(items) => items
            .iter()
            .filter_map(Value::as_str)
            .take(40)
            .map(str::to_string)
            .collect::<Vec<_>>(),
        None => return Ok(house_error(StatusCode::BAD_REQUEST, "hrefs required")),
    };
    let rows = match resolve_titles(&state.pg, &user.id, &hrefs).await {
        Ok(rows) => rows,
        Err(e) => return Ok(internal("[chips] resolve failed", e)),
    };
    let chips: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                "href": row.href,
                "entity": row.entity,
                "title": row.title,
            })
        })
        .collect();
    Ok(Json(json!({ "chips": chips })).into_response())
}
