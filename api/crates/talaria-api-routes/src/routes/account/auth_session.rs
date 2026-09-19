// GET /api/auth/session. The current user + their denied views + effective
// permissions, read from the DB each time so an admin's access change applies
// without re-login. No session is NOT an error here:
// {user: null, deniedViews: [], perms: []}.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use talaria_error::thrown_internal_error;
use talaria_session::{SessionUser, get_session_user};
use talaria_state::AppState;
use talaria_users as users;

#[derive(serde::Serialize)]
struct SessionBody {
    user: Option<SessionUser>,
    #[serde(rename = "deniedViews")]
    denied_views: Vec<String>,
    perms: Vec<String>,
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match get_session_user(&state, &headers).await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("[auth/session] redis read failed: {e}");
            return thrown_internal_error();
        }
    };
    // Either read failing 500s the route.
    let (denied, perms) = match &user {
        Some(u) => {
            let denied = users::denied_views(&state.pg, &u.id, &u.role).await;
            let perms = users::user_permissions(&state.pg, &u.id, &u.role).await;
            match (denied, perms) {
                (Ok(d), Ok(p)) => (d, p.into_iter().map(String::from).collect()),
                _ => {
                    tracing::error!("[auth/session] permission/view read failed");
                    return thrown_internal_error();
                }
            }
        }
        None => (Vec::new(), Vec::new()),
    };
    Json(SessionBody {
        user,
        denied_views: denied,
        perms,
    })
    .into_response()
}
