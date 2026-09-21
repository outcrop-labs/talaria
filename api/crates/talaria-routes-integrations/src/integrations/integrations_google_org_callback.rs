// GET /api/integrations/google/org/callback — store the SHARED org
// connection. The shared connect body plus the org's two differences: only an
// admin may tie the org's containers to a Google account, and the landing
// page is the admin panel whose googleOrg flash reads the status param.

use axum::extract::State;
use axum::http::{HeaderMap, Uri};
use axum::response::Response;

use talaria_api_facades::google::oauth::{ConnectFlavor, handle_connect_callback};
use talaria_body::percent_encode;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    handle_connect_callback(
        &state,
        &headers,
        &uri,
        ConnectFlavor::Org,
        "integrations/google/org",
        |status| format!("/admin?googleOrg={}", percent_encode(status)),
    )
    .await
}
