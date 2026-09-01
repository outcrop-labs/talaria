// GET /api/integrations/google/callback — port of
// ui/src/routes/api/integrations/google.callback.ts. Verify state, exchange
// the code for an offline refresh token, and store the connection for the
// signed-in user. The flow itself (gate, state, exchange, bounce-back) is the
// shared connect body; what THIS route adds is the meaning — the tokens are
// THIS user's, and the human lands on the integrations tab whose flash reads
// the status param (the CONNECTIONS tab specifically: that flash is mounted by
// IntegrationsSection, which only exists there).

use axum::extract::State;
use axum::http::{HeaderMap, Uri};
use axum::response::Response;

use crate::google_oauth::{ConnectFlavor, encode_uri_component, handle_connect_callback};
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    handle_connect_callback(
        &state,
        &headers,
        &uri,
        ConnectFlavor::Personal,
        "integrations/google",
        |status| {
            format!(
                "/settings/connections?google={}",
                encode_uri_component(status)
            )
        },
    )
    .await
}
