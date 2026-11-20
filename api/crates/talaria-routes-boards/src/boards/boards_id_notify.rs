// /api/boards/{id}/notify — the caller's own notification setting for one
// board (talaria_notify::boards; docs/notifications/producers.md "Each
// board"). Sessions only: a setting is a person's, and humans are the only
// recipients of ticket rows in v1.
//
//   GET    → { effective, source, stored, default, registry }: the setting
//            that applies to the caller (always expanded: preset, events,
//            scope, cadence), where it came from ("board" | "default" |
//            "builtin"), their own row for this board or null, their default
//            for every board (the same {effective, source, stored} shape),
//            and the vocabulary the client renders — labels included.
//   PUT    { preset, events?, scope?, cadence? } → the caller's own row for
//            this board, replaced (an upsert on their row — never anyone
//            else's); answers as GET. `events` and `scope` are Custom's
//            alone; an event with no producer yet is refused. A `digest`
//            cadence puts the board on the daily board digest.
//   DELETE → the caller's row is gone and the board follows their default
//            again; answers as GET.
//
// Any board role may call it — a viewer chooses what they hear about a board
// they can read — and anyone else is refused the way a board they cannot see
// is everywhere: 403. A setting survives leaving the board (it is dormant
// while they cannot see it, because every producer re-checks access at
// fan-out) and applies again if they come back; it goes when the person or
// the board is deleted.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use talaria_boards::board_role;
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::boards::{self as board_prefs, BoardPref};
use talaria_session::require_user;
use talaria_state::AppState;

/// The caller's session and their role on the board, or the refusal.
async fn gate(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    action: &str,
) -> Result<String, Response> {
    let user = require_user(state, headers).await?;
    if let Some(gate) = talaria_params::uuid_gate_404(id) {
        return Err(gate);
    }
    match board_role(&state.pg, &user.id, id).await {
        Ok(Some(_)) => Ok(user.id),
        Ok(None) => Err(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => Err(internal(
            &format!("[boards] role read on {action} notify failed"),
            e,
        )),
    }
}

/// What every verb answers: the state a reload would GET.
async fn answer(state: &AppState, user_id: &str, board_id: &str) -> Response {
    let stored = match board_prefs::stored(&state.pg, user_id, board_id).await {
        Ok(v) => v,
        Err(e) => return internal("[boards] notify setting read failed", e),
    };
    let default = match board_prefs::default_for(&state.pg, user_id).await {
        Ok(v) => v,
        Err(e) => return internal("[boards] notify default read failed", e),
    };
    Json(wire(stored.as_ref(), default.as_ref())).into_response()
}

/// The answer's shape, from the caller's row and default. Pure.
fn wire(stored: Option<&BoardPref>, default: Option<&BoardPref>) -> Value {
    let (effective, source) = board_prefs::effective(stored, default);
    let (default_effective, default_source) = board_prefs::effective(None, default);
    let mut out = board_prefs::state_wire(&effective, source, stored);
    out["default"] = board_prefs::state_wire(&default_effective, default_source, default);
    out["registry"] = board_prefs::registry_wire();
    out
}

// doc: The caller's own setting for this board: `{effective, source, stored, default, registry}`. Any board role.
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user_id = gate(&state, &headers, &id, "GET").await?;
    Ok(answer(&state, &user_id, &id).await)
}

// doc: `{preset, events?, scope?, cadence?}` → replaces the caller's own setting for this board; answers as GET. `events` and `scope` only with `custom`.
pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user_id = gate(&state, &headers, &id, "PUT").await?;
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let pref = match board_prefs::parse_write(&Value::Object(obj.clone())) {
        Ok(p) => p,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Err(e) = board_prefs::store(&state.pg, &user_id, &id, &pref).await {
        return Ok(internal("[boards] notify setting write failed", e));
    }
    Ok(answer(&state, &user_id, &id).await)
}

// doc: The caller's own setting for this board is removed — it follows their default again; answers as GET.
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user_id = gate(&state, &headers, &id, "DELETE").await?;
    if let Err(e) = board_prefs::clear(&state.pg, &user_id, &id).await {
        return Ok(internal("[boards] notify setting delete failed", e));
    }
    Ok(answer(&state, &user_id, &id).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use talaria_notify::boards::{Cadence, Preset};

    #[test]
    fn the_answer_names_the_layer_that_applies() {
        let off = BoardPref::preset(Preset::Off, Cadence::Instant);
        let all = BoardPref::preset(Preset::All, Cadence::Instant);
        // Nothing stored: the built-in, twice.
        let w = wire(None, None);
        assert_eq!(w["source"], "builtin");
        assert_eq!(w["stored"], Value::Null);
        assert_eq!(w["effective"]["preset"], "participating");
        assert_eq!(w["default"]["source"], "builtin");
        // A default and no row: the default applies.
        let w = wire(None, Some(&all));
        assert_eq!(
            (w["source"].as_str(), w["effective"]["preset"].as_str()),
            (Some("default"), Some("all"))
        );
        assert_eq!(
            w["default"]["stored"],
            json!({ "preset": "all", "cadence": "instant" })
        );
        // A row wins over the default, and the default is still shown.
        let w = wire(Some(&off), Some(&all));
        assert_eq!(w["source"], "board");
        assert_eq!(w["effective"]["events"], json!([]));
        assert_eq!(
            w["stored"],
            json!({ "preset": "off", "cadence": "instant" })
        );
        assert_eq!(w["default"]["effective"]["preset"], "all");
        assert!(w["registry"]["events"].is_array());
        // The key order is the wire contract.
        assert_eq!(
            w.as_object().unwrap().keys().cloned().collect::<Vec<_>>(),
            ["effective", "source", "stored", "default", "registry"]
        );
    }
}
