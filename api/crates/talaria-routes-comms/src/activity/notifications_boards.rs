// GET /api/notifications/boards — the caller's notification setting for
// every board they can see, with their default for every board, for the
// Settings → Notifications "Boards" list (talaria_notify::boards):
//
//   { default: {effective, source, stored},
//     boards: [{id, name, role, effective, source}] }
//
// `source` says where each board's effective setting comes from ("board": the
// caller's own setting for it; "default": their default; "builtin": neither).
// The boards are the ones the caller's board list shows (live, not
// archived), in its order — a setting for a board they can no longer see
// stays stored, dormant, and is not listed. Sessions only.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_boards::list_boards;
use talaria_error::internal;
use talaria_notify::boards::{self as board_prefs, BoardPref};
use talaria_session::require_user;
use talaria_state::AppState;

/// One listed board. Pure.
fn board_row(
    id: &str,
    name: &str,
    role: Option<&str>,
    stored: Option<&BoardPref>,
    default: Option<&BoardPref>,
) -> Value {
    let (effective, source) = board_prefs::effective(stored, default);
    json!({
        "id": id,
        "name": name,
        "role": role,
        "effective": effective.to_wire(),
        "source": source.id(),
    })
}

// doc: The caller's default for every board and the effective setting of every board they can see: `{default, boards: [{id, name, role, effective, source}]}`.
pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let boards = match list_boards(&state.pg, &user.id, false).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[notifications] board list failed", e)),
    };
    let ids: Vec<String> = boards.iter().map(|b| b.id.clone()).collect();
    let stored = match board_prefs::stored_for_boards(&state.pg, &user.id, &ids).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[notifications] board settings read failed", e)),
    };
    let default = match board_prefs::default_for(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[notifications] board default read failed", e)),
    };
    let (default_effective, default_source) = board_prefs::effective(None, default.as_ref());
    let rows: Vec<Value> = boards
        .iter()
        .map(|b| {
            board_row(
                &b.id,
                &b.name,
                b.role.as_deref(),
                stored.get(&b.id),
                default.as_ref(),
            )
        })
        .collect();
    Ok(Json(json!({
        "default": board_prefs::state_wire(&default_effective, default_source, default.as_ref()),
        "boards": rows,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_notify::boards::{Cadence, Preset};

    #[test]
    fn a_row_says_where_its_setting_comes_from() {
        let off = BoardPref::preset(Preset::Off, Cadence::Instant);
        let mentions = BoardPref::preset(Preset::Mentions, Cadence::Instant);
        let r = board_row("b1", "Ops", Some("viewer"), None, None);
        assert_eq!(r["source"], "builtin");
        assert_eq!(r["effective"]["preset"], "participating");
        let r = board_row("b1", "Ops", Some("viewer"), None, Some(&mentions));
        assert_eq!(r["source"], "default");
        let r = board_row("b1", "Ops", Some("editor"), Some(&off), Some(&mentions));
        assert_eq!(
            (r["source"].as_str(), r["effective"]["preset"].as_str()),
            (Some("board"), Some("off"))
        );
        // Each board's own channels are listed with it (null: the matrix).
        assert_eq!(
            r["effective"]["channels"],
            json!({ "alerts": null, "email": null })
        );
        let mailed = mentions
            .clone()
            .with_channels(talaria_notify::boards::BoardChannels {
                email: Some(true),
                ..Default::default()
            });
        let r = board_row("b1", "Ops", Some("editor"), Some(&mailed), None);
        assert_eq!(r["effective"]["channels"]["email"], json!(true));
        assert_eq!(
            r.as_object().unwrap().keys().cloned().collect::<Vec<_>>(),
            ["id", "name", "role", "effective", "source"]
        );
    }
}
