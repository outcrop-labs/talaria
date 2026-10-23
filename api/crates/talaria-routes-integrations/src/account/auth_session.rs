// GET /api/auth/session. The current user + their denied views + the app
// views they hold + effective permissions, read from the DB each time so an
// admin's access change applies without re-login. The user's face —
// effective picture, status emoji and text — is read fresh from the users row
// the same way, so a photo or status set in another tab (or before this
// session existed) shows here. No session is NOT an error here:
// {user: null, deniedViews: [], appGrants: [], appsUnreadable: false, perms: []}.
//
// `appGrants` is what the app gates admit on (ui/src/lib/app-grants.ts): the
// app views this person holds, listed positively. A view's absence from
// `deniedViews` is never read as a grant. When the enabled apps cannot be
// read (a malformed apps_enabled, an apps directory that cannot be listed),
// the route still answers 200 with the core `deniedViews` and `perms`, and
// `appGrants: null, appsUnreadable: true`: every app reader holds that as no
// app, and the app doors answer 503. A broken app setting costs a member
// their apps, not all of Talaria.

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
    /// `None` (null) when the enabled apps could not be read.
    #[serde(rename = "appGrants")]
    app_grants: Option<Vec<String>>,
    #[serde(rename = "appsUnreadable")]
    apps_unreadable: bool,
    perms: Vec<String>,
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match get_session_user(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return internal("[auth/session] redis read failed", e),
    };
    // Either read failing 500s the route. The apps read is not one of them:
    // `view_grants` withholds the app grants (None) when it fails, and the
    // core views and permissions still answer.
    let (grants, perms) = match &user {
        Some(u) => {
            let grants = match users::view_grants(&state.pg, &u.id, &u.role).await {
                Ok(g) => g,
                Err(e) => return internal("[auth/session] view grants read failed", e),
            };
            match users::user_permissions(&state.pg, &u.id, &u.role).await {
                Ok(p) => (grants, p.into_iter().map(String::from).collect()),
                Err(e) => return internal("[auth/session] permission read failed", e),
            }
        }
        None => (
            users::ViewGrants {
                denied: Vec::new(),
                app_grants: Some(Vec::new()),
            },
            Vec::new(),
        ),
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
        denied_views: grants.denied,
        apps_unreadable: grants.app_grants.is_none(),
        app_grants: grants.app_grants,
        perms,
    })
    .into_response()
}
