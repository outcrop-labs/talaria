// GET /api/auth/session. The current user + their denied views + effective
// permissions, read from the DB each time so an admin's access change applies
// without re-login. The user's face — effective picture, status emoji and
// text — is read fresh from the users row the same way, so a photo or status
// set in another tab (or before this session existed) shows here. No session
// is NOT an error here: {user: null, deniedViews: [], perms: []}.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use talaria_error::internal;
use talaria_session::{SessionUserWire, get_session_user};
use talaria_state::AppState;
use talaria_users as users;

#[derive(serde::Serialize)]
struct SessionBody {
    user: Option<SessionUserWire>,
    #[serde(rename = "deniedViews")]
    denied_views: Vec<String>,
    perms: Vec<String>,
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match get_session_user(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return internal("[auth/session] redis read failed", e),
    };
    // Either read failing 500s the route.
    let (denied, perms) = match &user {
        Some(u) => {
            let denied = users::denied_views(&state.pg, &u.id, &u.role).await;
            let perms = users::user_permissions(&state.pg, &u.id, &u.role).await;
            match (denied, perms) {
                (Ok(d), Ok(p)) => (d, p.into_iter().map(String::from).collect()),
                _ => return internal("[auth/session]", "permission/view read failed"),
            }
        }
        None => (Vec::new(), Vec::new()),
    };
    let user = match user {
        Some(mut u) => match users::profile_face(&state.pg, &u.id).await {
            Ok(face) => {
                let face = face.unwrap_or(users::ProfileFace {
                    picture: u.picture.clone(),
                    status_emoji: None,
                    status_text: None,
                });
                u.picture = face.picture;
                Some(SessionUserWire {
                    user: u,
                    status_emoji: face.status_emoji,
                    status_text: face.status_text,
                })
            }
            Err(e) => return internal("[auth/session] profile read failed", e),
        },
        None => None,
    };
    Json(SessionBody {
        user,
        denied_views: denied,
        perms,
    })
    .into_response()
}
